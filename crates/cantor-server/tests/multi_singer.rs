//! One render call mixes tracks sung by different backends: a DiffSinger
//! voicebank and a UTAU voicebank (generated, cantor-fixtures::utau).

use cantor_core::*;
use cantor_render::render::Quality;
use cantor_server::{Engine, EngineConfig};
use std::collections::BTreeMap;
use std::path::Path;

const QIXUAN: &str = "Qixuan_v2.7.0_DiffSinger_OpenUtau";

fn engine() -> Engine {
    let vb = cantor_fixtures::ensure(&cantor_fixtures::QIXUAN).unwrap();
    cantor_fixtures::ensure(&cantor_fixtures::PC_NSF_HIFIGAN).unwrap();
    let rmvpe = cantor_fixtures::ensure(&cantor_fixtures::RMVPE).unwrap();
    let work = Path::new(env!("CARGO_TARGET_TMPDIR")).join("multi-singer");
    let _ = std::fs::remove_dir_all(&work);
    let utau_dir = work.join("singers");
    cantor_fixtures::utau::write_synthetic_voicebank(&utau_dir.join("synthetic-cv")).unwrap();
    Engine::new(EngineConfig {
        singer_paths: vec![vb.parent().unwrap().to_path_buf(), utau_dir],
        vocoder_paths: vec![cantor_fixtures::dir()],
        phonemizer_paths: vec![],
        rmvpe: Some(rmvpe),
        cache_dir: work.join("cache"),
        render_dir: work.join("renders"),
    })
}

fn track(id: u64, singer: &str, backend: &str, lyrics: &[&str], base: f64) -> Track {
    let notes = lyrics
        .iter()
        .enumerate()
        .map(|(i, l)| Note {
            id: NoteId(id * 100 + i as u64),
            start: i as i64 * 1920,
            length: 1920,
            pitch_cents: base + 200.0 * i as f64,
            lyric: (*l).into(),
            lang: None,
            phonemes: None,
            expr: NoteExpr::default(),
            modes: VocalModes::default(),
        })
        .collect();
    Track {
        id: TrackId(id),
        name: singer.into(),
        singer: SingerRef {
            id: singer.into(),
            version: None,
        },
        backend: BackendRef(backend.into()),
        mix: Mix::default(),
        effects: vec![],
        doubling: Doubling::default(),
        voice_mix: vec![],
        curves: BTreeMap::new(),
        parts: vec![Part {
            id: PartId(id * 10),
            name: "A".into(),
            start: 0,
            length: 4 * 1920,
            notes,
        }],
    }
}

#[test]
fn diffsinger_and_utau_tracks_render_in_one_call() {
    let e = engine();
    let singers = e.list_singers();
    let backend_of = |id: &str| {
        singers
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.backend.clone())
    };
    assert_eq!(backend_of(QIXUAN).as_deref(), Some("diffsinger"));
    assert_eq!(backend_of("synthetic-cv").as_deref(), Some("utau"));
    let schema = e.get_singer_schema("synthetic-cv").unwrap();
    let supported: Vec<&str> = schema
        .parameters
        .iter()
        .filter(|p| p.supported)
        .map(|p| p.descriptor.id)
        .collect();
    assert_eq!(supported, ["pitch", "energy", "breathiness", "gender"]);

    let mut p = Project::new();
    p.tracks
        .push(track(1, QIXUAN, "diffsinger", &["la", "la", "la"], 6200.0));
    p.tracks.push(track(
        2,
        "synthetic-cv",
        "utau",
        &["ka", "sa", "na"],
        5500.0,
    ));
    let info = e.add_project(p);
    let r = e.render(&info.project_id, None, Quality::Preview).unwrap();
    assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
    let analysis = r.analysis.unwrap();
    assert_eq!(analysis.tracks.len(), 2);
    for t in &analysis.tracks {
        let p95 = t.pitch.p95_cents.expect("each track is measured");
        assert!(p95 < 30.0, "{:?}: p95 {p95:.1} cents", t.track);
    }
    let described = e.describe_project(&info.project_id).unwrap();
    assert!(described.contains("ka"), "{described}");
}
