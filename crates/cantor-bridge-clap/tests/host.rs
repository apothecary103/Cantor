//! Hosts the plugin in-process (clack-host) the way a DAW would: Cantor
//! publishes a render to the channel, the host plays from a song position
//! at another sample rate, and the plugin's output must be that render at
//! that position. The host's transport must reach Cantor's side.
//!
//! One test per binary: it points the plugin at a private channel directory
//! through `CANTOR_BRIDGE_DIR` before any plugin is created.

use cantor_bridge::{Channel, DEFAULT_CHANNEL, channel_path};
use cantor_bridge_clap::CantorBridge;
use clack_extensions::audio_ports::{AudioPortInfoBuffer, PluginAudioPorts};
use clack_host::events::EventFlags;
use clack_host::events::event_types::{TransportEvent, TransportFlags};
use clack_host::factory::plugin::PluginFactory;
use clack_host::prelude::*;
use clack_host::utils::{BeatTime, SecondsTime};
use clack_plugin::entry::SinglePluginEntry;

struct Shared;
struct Main;
struct Proc;
struct Handlers;

impl SharedHandler<'_> for Shared {
    fn request_restart(&self) {}
    fn request_process(&self) {}
    fn request_callback(&self) {}
}
impl AudioProcessorHandler<'_> for Proc {}
impl MainThreadHandler<'_> for Main {}
impl HostHandlers for Handlers {
    type Shared<'a> = Shared;
    type MainThread<'a> = Main;
    type AudioProcessor<'a> = Proc;
}

const SRC_RATE: u32 = 44_100;
const HOST_RATE: f64 = 48_000.0;
const BLOCK: usize = 256;

/// The published "render": a slow ramp on the left, its negation on the
/// right, so positions and channels are both checkable.
fn render_sample(frame: usize) -> (f32, f32) {
    let v = frame as f32 / 100_000.0;
    (v, -v)
}

fn transport(playing: bool, secs: f64, tempo: f64) -> TransportEvent {
    let mut flags = TransportFlags::HAS_TEMPO
        | TransportFlags::HAS_SECONDS_TIMELINE
        | TransportFlags::HAS_BEATS_TIMELINE;
    if playing {
        flags |= TransportFlags::IS_PLAYING;
    }
    TransportEvent {
        header: EventHeader::new_core(0, EventFlags::empty()),
        flags,
        song_pos_beats: BeatTime::from_float(secs * tempo / 60.0),
        song_pos_seconds: SecondsTime::from_float(secs),
        tempo,
        tempo_inc: 0.0,
        loop_start_beats: BeatTime::from_int(0),
        loop_end_beats: BeatTime::from_int(0),
        loop_start_seconds: SecondsTime::from_int(0),
        loop_end_seconds: SecondsTime::from_int(0),
        bar_start: BeatTime::from_int(0),
        bar_number: 0,
        time_signature_numerator: 4,
        time_signature_denominator: 4,
    }
}

#[test]
fn plays_the_published_render_at_the_host_position() {
    let dir = std::env::temp_dir().join(format!("cantor-bridge-host-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // SAFETY: the only test in this binary sets the variable before any
    // other thread reads the environment.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("CANTOR_BRIDGE_DIR", &dir);
    }

    // Cantor's side: publish 10 s of audio.
    let engine = Channel::open(&channel_path(DEFAULT_CHANNEL)).unwrap();
    let frames = 10 * SRC_RATE as usize;
    let audio: Vec<f32> = (0..frames)
        .flat_map(|f| {
            let (l, r) = render_sample(f);
            [l, r]
        })
        .collect();
    engine.region().publish(SRC_RATE, &audio).unwrap();

    // The DAW's side.
    let info = HostInfo::new("cantor-test-host", "Cantor", "", "0").unwrap();
    let entry = PluginEntry::load_from_clack::<SinglePluginEntry<CantorBridge>>(c"").unwrap();
    let factory = entry.get_factory::<PluginFactory>().unwrap();
    let descriptor = factory.plugin_descriptor(0).unwrap();
    assert_eq!(descriptor.id().unwrap().to_bytes(), b"org.cantor.bridge");
    let mut plugin = PluginInstance::<Handlers>::new(
        |_| Shared,
        |_| Main,
        &entry,
        descriptor.id().unwrap(),
        &info,
    )
    .unwrap();
    let handle = plugin.plugin_handle();
    let ports = handle.get_extension::<PluginAudioPorts>().unwrap();
    assert_eq!(ports.count(&handle, true), 0);
    assert_eq!(ports.count(&handle, false), 1);
    let mut buf = AudioPortInfoBuffer::new();
    assert_eq!(
        ports
            .get(&handle, 0, false, &mut buf)
            .unwrap()
            .channel_count,
        2
    );

    let processor = plugin
        .activate(
            |_, _| Proc,
            PluginAudioConfiguration {
                sample_rate: HOST_RATE,
                min_frames_count: 1,
                max_frames_count: BLOCK as u32,
            },
        )
        .unwrap();
    let mut processor = processor.start_processing().unwrap();
    let mut run = |t: &TransportEvent| -> [Vec<f32>; 2] {
        let mut out = [vec![9.0f32; BLOCK], vec![9.0f32; BLOCK]];
        let mut outs = AudioPorts::with_capacity(2, 1);
        let mut out_bufs = outs.with_output_buffers([AudioPortBuffer {
            channels: AudioPortBufferType::f32_output_only(out.iter_mut().map(Vec::as_mut_slice)),
            latency: 0,
        }]);
        let ins = InputAudioBuffers::empty();
        let input_events = EventBuffer::new();
        let mut output_events = EventBuffer::new();
        processor
            .process(
                &ins,
                &mut out_bufs,
                &input_events.as_input(),
                &mut output_events.as_output(),
                None,
                Some(t),
            )
            .unwrap();
        out
    };

    // Playing from 2.5 s: sample i is the render at 2.5 s + i / 48 kHz.
    let start = 2.5;
    let [l, r] = run(&transport(true, start, 96.0));
    for i in 0..BLOCK {
        let src = (start + i as f64 / HOST_RATE) * f64::from(SRC_RATE);
        let expect = src as f32 / 100_000.0;
        assert!(
            (l[i] - expect).abs() < 1e-5,
            "frame {i}: {} vs {expect}",
            l[i]
        );
        assert!((r[i] + expect).abs() < 1e-5);
    }
    // Stopped: silence.
    let [l, _] = run(&transport(false, start, 96.0));
    assert!(l.iter().all(|&x| x == 0.0));

    // Cantor sees the host's transport.
    let host = engine
        .region()
        .read_host()
        .expect("the plugin reported transport");
    assert!(!host.playing);
    assert_eq!(host.tempo, Some(96.0));
    assert_eq!(host.sample_rate, HOST_RATE);
    assert!((host.position_secs - start).abs() < 1e-9);
    assert_eq!(host.heartbeat, 2);

    plugin.deactivate(processor.stop_processing());
    let _ = std::fs::remove_dir_all(dir);
}
