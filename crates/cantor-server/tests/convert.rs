//! Audio-to-voice conversion: a recording (here a UTAU render of a known
//! melody) becomes a project another voice sings, only with the
//! performer's consent; cloned voices without an attestation are refused.

use cantor_core::*;
use cantor_render::render::Quality;
use cantor_server::convert::ConsentAttestation;
use cantor_server::{Engine, EngineConfig, EngineError};
use std::path::{Path, PathBuf};

fn setup() -> (Engine, PathBuf) {
    let work = Path::new(env!("CARGO_TARGET_TMPDIR")).join("convert");
    let _ = std::fs::remove_dir_all(&work);
    let singers = work.join("singers");
    cantor_fixtures::utau::write_synthetic_voicebank(&singers.join("synthetic-cv")).unwrap();
    // The same bank, declared a clone of a real voice, with and without an
    // attestation.
    for (name, yaml) in [
        ("clone-unattested", "cloned: true\n"),
        (
            "clone-attested",
            "cloned: true\nconsent_attestation: signed 2026-10-01\n",
        ),
    ] {
        let dir = singers.join(name);
        cantor_fixtures::utau::write_synthetic_voicebank(&dir).unwrap();
        std::fs::write(dir.join("character.yaml"), yaml).unwrap();
    }
    let engine = Engine::new(EngineConfig {
        singer_paths: vec![singers],
        vocoder_paths: vec![],
        phonemizer_paths: vec![],
        rmvpe: Some(cantor_fixtures::ensure(&cantor_fixtures::RMVPE).unwrap()),
        cache_dir: work.join("cache"),
        render_dir: work.join("renders"),
    });
    (engine, work)
}

const MELODY: [(i64, f64, &str); 4] = [
    (0, 60.0, "ka"),
    (1, 64.0, "sa"),
    (2, 67.0, "na"),
    (4, 62.0, "ra"),
];

fn recording(engine: &Engine) -> PathBuf {
    let notes = MELODY
        .iter()
        .enumerate()
        .map(|(i, &(beat, midi, lyric))| Note {
            id: NoteId(10 + i as u64),
            start: beat * 1920,
            length: 1920,
            pitch_cents: midi * 100.0,
            lyric: lyric.into(),
            lang: None,
            phonemes: None,
            expr: NoteExpr::default(),
            modes: VocalModes::default(),
        })
        .collect();
    let mut p = Project::new();
    p.tracks.push(Track {
        id: TrackId(1),
        name: "Performer".into(),
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
            length: 6 * 1920,
            notes,
        }],
    });
    let id = engine.add_project(p).project_id;
    let r = engine.render(&id, None, Quality::Preview).unwrap();
    PathBuf::from(r.audio_uri.trim_start_matches("file://"))
}

fn consent() -> ConsentAttestation {
    ConsentAttestation {
        performer: "Synthetic Performer".into(),
        statement: "I agree that this recording may be re-sung by Cantor voices.".into(),
        date: "2026-10-25".into(),
        confirmed: true,
    }
}

#[test]
fn converts_with_consent_and_refuses_without() {
    let (engine, _work) = setup();
    let wav = recording(&engine);
    let lyrics: Vec<String> = ["a", "i", "u", "e"].iter().map(|s| s.to_string()).collect();

    // No consent, no conversion.
    let refused = engine.convert_audio(
        &wav,
        "synthetic-cv",
        &lyrics,
        &ConsentAttestation {
            confirmed: false,
            ..consent()
        },
    );
    assert!(matches!(refused, Err(EngineError::Invalid(ref m)) if m.contains("consent")));

    let info = engine
        .convert_audio(&wav, "synthetic-cv", &lyrics, &consent())
        .unwrap();
    let (p, _) = engine.snapshot(&info.project_id).unwrap();
    let notes = &p.tracks[0].parts[0].notes;
    let pitches: Vec<f64> = notes.iter().map(|n| n.pitch_cents).collect();
    let expected: Vec<f64> = MELODY.iter().map(|m| m.1 * 100.0).collect();
    assert_eq!(pitches, expected, "{notes:?}");
    for (n, m) in notes.iter().zip(MELODY) {
        let onset_secs = (n.start as f64 / 1920.0) * 0.5;
        assert!((onset_secs - m.0 as f64 * 0.5).abs() < 0.1, "{n:?}");
    }
    assert_eq!(notes[0].lyric, "a");
    assert_eq!(p.meta.provenance.len(), 1);
    assert_eq!(p.meta.provenance[0].performer, "Synthetic Performer");
    assert!(p.tracks[0].curves.contains_key("pitch"));

    // The converted project is sung back in tune.
    let r = engine
        .render(&info.project_id, None, Quality::Preview)
        .unwrap();
    let p95 = r.analysis.unwrap().tracks[0].pitch.p95_cents.unwrap();
    assert!(p95 < 30.0, "p95 {p95:.1} cents");

    // A cloned voice needs its own attestation.
    let e = engine
        .convert_audio(&wav, "clone-unattested", &lyrics, &consent())
        .unwrap_err();
    assert!(e.to_string().contains("cloned voice"), "{e}");
    engine
        .convert_audio(&wav, "clone-attested", &lyrics, &consent())
        .unwrap();
}
