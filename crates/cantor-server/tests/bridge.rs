//! The engine publishes renders to a DAW bridge channel and reads back the
//! transport the plugin reports. (The plugin side is tested in
//! cantor-bridge-clap with a real CLAP host.) One test per binary: it sets
//! `CANTOR_BRIDGE_DIR` before anything reads it.

use cantor_bridge::{Channel, HostState, channel_path};
use cantor_core::*;
use cantor_render::render::Quality;
use cantor_server::{Engine, EngineConfig};
use std::path::Path;

#[test]
fn publish_and_status_round_trip_through_the_channel() {
    let work = Path::new(env!("CARGO_TARGET_TMPDIR")).join("bridge");
    let _ = std::fs::remove_dir_all(&work);
    // SAFETY: the only test in this binary; no other thread reads the
    // environment yet.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("CANTOR_BRIDGE_DIR", work.join("channels"));
    }
    let singers = work.join("singers");
    cantor_fixtures::utau::write_synthetic_voicebank(&singers.join("synthetic-cv")).unwrap();
    let engine = Engine::new(EngineConfig {
        singer_paths: vec![singers],
        vocoder_paths: vec![],
        phonemizer_paths: vec![],
        rmvpe: Some(cantor_fixtures::ensure(&cantor_fixtures::RMVPE).unwrap()),
        cache_dir: work.join("cache"),
        render_dir: work.join("renders"),
    });
    let mut p = Project::new();
    p.tracks.push(Track {
        id: TrackId(1),
        name: "Lead".into(),
        singer: SingerRef {
            id: "synthetic-cv".into(),
            version: None,
        },
        backend: BackendRef("utau".into()),
        mix: Mix::default(),
        effects: vec![],
        doubling: Doubling::default(),
        voice_mix: vec![],
        curves: Default::default(),
        parts: vec![Part {
            id: PartId(2),
            name: "A".into(),
            start: 0,
            length: 2 * 1920,
            notes: vec![Note {
                id: NoteId(3),
                start: 0,
                length: 1920,
                pitch_cents: 6000.0,
                lyric: "ka".into(),
                lang: None,
                phonemes: None,
                expr: NoteExpr::default(),
                modes: VocalModes::default(),
            }],
        }],
    });
    let id = engine.add_project(p).project_id;

    let before = engine.bridge_status(None).unwrap();
    assert!(!before.plugin_seen && before.published_generation.is_none());

    let published = engine.bridge_publish(&id, None, Quality::Preview).unwrap();
    assert_eq!(published.channel, "default");
    assert!(published.seconds > 0.4);
    // The channel holds exactly the rendered WAV.
    let render = engine.get_render(&published.render_id).unwrap();
    let (wav, sr) = cantor_render::wav::read_stereo_f32(Path::new(
        render.audio_uri.trim_start_matches("file://"),
    ))
    .unwrap();
    let ch = Channel::open(&channel_path("default")).unwrap();
    let mut out = vec![0.0f32; wav.len()];
    assert_eq!(
        ch.region().read(0, &mut out),
        Some((sr, published.generation))
    );
    assert_eq!(out, wav);

    // What the plugin reports shows up in the status.
    ch.region().write_host(&HostState {
        playing: true,
        sample_rate: 48_000.0,
        tempo: Some(100.0),
        position_secs: 1.25,
        position_beats: 2.08,
        heartbeat: 7,
    });
    let s = engine.bridge_status(None).unwrap();
    assert!(s.plugin_seen && s.playing);
    assert_eq!(s.host_tempo, Some(100.0));
    assert_eq!(s.published_generation, Some(published.generation));
    assert_eq!(s.heartbeat, 7);
}
