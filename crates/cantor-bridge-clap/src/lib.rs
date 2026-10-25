//! The Cantor bridge plugin (CLAP; VST3 and AU through clap-wrapper).
//!
//! An instrument with one stereo output and no GUI. It maps the bridge
//! channel (`cantor_bridge::Channel`, the `default` channel in
//! `cantor_bridge::channel_dir()`), and while the host plays it outputs
//! the audio Cantor last published at the host's song position, resampled
//! to the host's rate. Every block it records the host's transport (playing,
//! position, tempo) in the channel for Cantor to read.
//!
//! The audio thread only reads and writes atomics and copies samples into
//! a buffer allocated in `activate`: no locks, allocation, logging or
//! system calls (CLAUDE.md).

use cantor_bridge::{Channel, DEFAULT_CHANNEL, HostState};
use clack_extensions::audio_ports::*;
use clack_plugin::events::event_types::TransportFlags;
use clack_plugin::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};

/// The plugin type.
#[derive(Debug)]
pub struct CantorBridge;

impl Plugin for CantorBridge {
    type AudioProcessor<'a> = Processor<'a>;
    type Shared<'a> = Shared;
    type MainThread<'a> = MainThread<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&Shared>) {
        builder.register::<PluginAudioPorts>();
    }
}

impl DefaultPluginFactory for CantorBridge {
    fn get_descriptor() -> PluginDescriptor {
        use clack_plugin::plugin::features::*;
        PluginDescriptor::new("org.cantor.bridge", "Cantor Bridge")
            .with_vendor("Cantor")
            .with_url("https://github.com/apothecary103/Cantor")
            .with_version(env!("CARGO_PKG_VERSION"))
            .with_description("Plays Cantor's vocal renders in sync with the host transport")
            .with_features([INSTRUMENT, STEREO])
    }

    fn new_shared(_host: HostSharedHandle<'_>) -> Result<Shared, PluginError> {
        // A missing or unwritable channel is not fatal: the plugin is
        // silent until Cantor publishes.
        Ok(Shared {
            channel: Channel::open_named(DEFAULT_CHANNEL).ok(),
            heartbeat: AtomicU64::new(0),
        })
    }

    fn new_main_thread<'a>(
        _host: HostMainThreadHandle<'a>,
        shared: &'a Shared,
    ) -> Result<MainThread<'a>, PluginError> {
        Ok(MainThread { _shared: shared })
    }
}

/// State shared by the plugin's threads.
#[derive(Debug)]
pub struct Shared {
    channel: Option<Channel>,
    heartbeat: AtomicU64,
}

impl PluginShared<'_> for Shared {}

/// Main-thread state.
#[derive(Debug)]
pub struct MainThread<'a> {
    _shared: &'a Shared,
}

impl<'a> PluginMainThread<'a, Shared> for MainThread<'a> {}

impl PluginAudioPortsImpl for MainThread<'_> {
    fn count(&self, is_input: bool) -> u32 {
        if is_input { 0 } else { 1 }
    }

    fn get(&self, index: u32, is_input: bool, writer: &mut AudioPortInfoWriter) {
        if !is_input && index == 0 {
            writer.set(&AudioPortInfo {
                id: ClapId::new(0),
                name: b"Cantor",
                channel_count: 2,
                flags: AudioPortFlags::IS_MAIN,
                port_type: Some(AudioPortType::STEREO),
                in_place_pair: None,
            });
        }
    }
}

/// Largest ratio of source to host sample rate supported (e.g. a 48 kHz
/// render in a 12 kHz host).
const MAX_RATIO: usize = 4;

/// The audio-thread state.
#[derive(Debug)]
pub struct Processor<'a> {
    shared: &'a Shared,
    host_rate: f64,
    /// Interleaved stereo scratch for one block of source frames.
    scratch: Vec<f32>,
}

impl<'a> PluginAudioProcessor<'a, Shared, MainThread<'a>> for Processor<'a> {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &MainThread<'a>,
        shared: &'a Shared,
        config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        let frames = config.max_frames_count as usize * MAX_RATIO + 4;
        Ok(Self {
            shared,
            host_rate: config.sample_rate,
            scratch: vec![0.0; frames * 2],
        })
    }

    fn process(
        &mut self,
        process: Process,
        mut audio: Audio,
        _events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        let mut port = audio
            .output_port(0)
            .ok_or(PluginError::Message("no output port"))?;
        let mut channels = port
            .channels()?
            .into_f32()
            .ok_or(PluginError::Message("expected f32 output"))?;
        let frames = channels.frames_count() as usize;

        let transport = process.transport;
        let playing = transport.is_some_and(|t| t.flags.contains(TransportFlags::IS_PLAYING));
        let tempo = transport
            .filter(|t| t.flags.contains(TransportFlags::HAS_TEMPO))
            .map(|t| t.tempo);
        let beats = transport
            .filter(|t| t.flags.contains(TransportFlags::HAS_BEATS_TIMELINE))
            .map(|t| t.song_pos_beats.to_float());
        let secs = transport.and_then(|t| {
            if t.flags.contains(TransportFlags::HAS_SECONDS_TIMELINE) {
                Some(t.song_pos_seconds.to_float())
            } else {
                Some(beats? * 60.0 / tempo?)
            }
        });

        let region = self.shared.channel.as_ref().map(Channel::region);
        if let Some(r) = &region {
            r.write_host(&HostState {
                playing,
                sample_rate: self.host_rate,
                tempo,
                position_secs: secs.unwrap_or(0.0),
                position_beats: beats.unwrap_or(0.0),
                heartbeat: self.shared.heartbeat.fetch_add(1, Ordering::Relaxed) + 1,
            });
        }

        // Source frames for this block, linearly interpolated.
        let mut got = false;
        if let (true, Some(start), Some(r)) = (playing, secs, &region)
            && let Some((src_rate, _, _)) = r.active_info()
            && start >= 0.0
        {
            let step = f64::from(src_rate) / self.host_rate;
            let pos = start * f64::from(src_rate);
            let first = pos.floor() as usize;
            let need = ((frames as f64 * step).ceil() as usize + 2).min(self.scratch.len() / 2);
            let buf = &mut self.scratch[..need * 2];
            if step <= MAX_RATIO as f64 && r.read(first, buf).is_some() {
                got = true;
                let frac0 = pos - first as f64;
                for (c, out) in channels.iter_mut().enumerate().take(2) {
                    for (i, o) in out.iter_mut().enumerate() {
                        let x = frac0 + i as f64 * step;
                        let j = (x.floor() as usize).min(need - 2);
                        let u = (x - j as f64) as f32;
                        *o = buf[2 * j + c] * (1.0 - u) + buf[2 * (j + 1) + c] * u;
                    }
                }
            }
        }
        if !got {
            for out in channels.iter_mut() {
                out.fill(0.0);
            }
        }
        Ok(ProcessStatus::Continue)
    }
}

clack_export_entry!(SinglePluginEntry<CantorBridge>);
