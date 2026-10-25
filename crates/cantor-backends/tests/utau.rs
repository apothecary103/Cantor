//! The UTAU backend with a generated CV voicebank (cantor-fixtures::utau)
//! and RMVPE: placement, pitch accuracy, determinism and the WORLD
//! vocoder fast path.

use cantor_backends::rmvpe::Rmvpe;
use cantor_backends::utau::Utau;
use cantor_core::*;
use cantor_render::cache::DiskCache;
use cantor_render::render::{Outcome, Quality, Renderer};
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

fn rmvpe() -> Arc<Rmvpe> {
    static R: OnceLock<Arc<Rmvpe>> = OnceLock::new();
    R.get_or_init(|| {
        let p = cantor_fixtures::ensure(&cantor_fixtures::RMVPE).expect("RMVPE fixture");
        Arc::new(Rmvpe::load(&p).expect("load RMVPE"))
    })
    .clone()
}

fn utau() -> Arc<Utau> {
    static U: OnceLock<Arc<Utau>> = OnceLock::new();
    U.get_or_init(|| {
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("synthetic-cv");
        cantor_fixtures::utau::write_synthetic_voicebank(&dir).unwrap();
        Arc::new(Utau::load(&dir, rmvpe()).expect("load voicebank"))
    })
    .clone()
}

/// Two phrases of CV syllables, a fifth apart and back.
fn project() -> Project {
    let melody: [(i64, f64, &str); 7] = [
        (0, 60.0, "ka"),
        (1, 62.0, "sa"),
        (2, 64.0, "na"),
        (3, 67.0, "ra"),
        (5, 67.0, "mo"),
        (6, 64.0, "ti"),
        (7, 60.0, "a"),
    ];
    let notes = melody
        .iter()
        .enumerate()
        .map(|(i, &(beat, midi, lyric))| Note {
            id: NoteId(100 + i as u64),
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
        curves: BTreeMap::new(),
        parts: vec![Part {
            id: PartId(2),
            name: "A".into(),
            start: 0,
            length: 9 * 1920,
            notes,
        }],
    });
    p
}

fn cache(name: &str) -> DiskCache {
    let d = std::env::temp_dir().join(format!("cantor-utau-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    DiskCache::new(d).unwrap()
}

#[test]
fn sings_the_score_in_tune_and_deterministically() {
    let r = Renderer::new(utau(), cache("tune")).with_analyzer(rmvpe());
    let p = project();
    let out = r.render(&p, Quality::Preview).unwrap();
    assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
    assert_eq!(out.trace.phrases.len(), 2);
    let report = out.analysis.as_ref().unwrap();
    let pitch = &report.tracks[0].pitch;
    let p95 = pitch.p95_cents.expect("measured");
    assert!(p95 < 20.0, "p95 pitch error {p95:.1} cents ({pitch:?})");
    let timing = &report.tracks[0].timing;
    let onsets: Vec<(&str, Option<f64>)> = report.tracks[0]
        .notes
        .iter()
        .map(|n| (n.lyric.as_str(), n.onset_error_ms))
        .collect();
    if let Some(t) = timing.p95_abs_ms {
        assert!(
            t < 60.0,
            "vowel onsets within 60 ms of note onsets: {timing:?} {onsets:?}"
        );
    }
    let wav = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("utau-melody.wav");
    cantor_render::wav::write_stereo_f32(&wav, out.sample_rate, &out.samples).unwrap();

    let cold = Renderer::new(utau(), cache("tune-cold"))
        .render(&p, Quality::Preview)
        .unwrap();
    assert_eq!(cold.audio_hash, out.audio_hash);
}

#[test]
fn pitch_edits_take_the_world_fast_path_and_doubling_warps_formants() {
    let r = Renderer::new(utau(), cache("fast"));
    let p = project();
    r.render(&p, Quality::Preview).unwrap();
    let mut doc = Document::new(p);
    let edit = doc.apply(Op::SetCurve {
        track: TrackId(1),
        param: ParamId::new("pitch"),
        range: TickRange {
            start: 0,
            end: 1920,
        },
        points: vec![(0, 30.0), (1919, 30.0)],
        interp: Interp::Linear,
    });
    assert!(edit.applied);
    let b = r.render(doc.project(), Quality::Preview).unwrap();
    assert_eq!(b.trace.phrases[0].outcome, Outcome::VocoderFastPath);
    assert_eq!(b.trace.phrases[1].outcome, Outcome::CacheHit);

    let doubled = doc.apply(Op::SetTrack {
        id: TrackId(1),
        patch: TrackPatch {
            doubling: Some(Doubling {
                layers: 3,
                ..Doubling::default()
            }),
            ..TrackPatch::default()
        },
    });
    assert!(doubled.applied);
    let c = r.render(doc.project(), Quality::Preview).unwrap();
    assert!(
        c.diagnostics.is_empty(),
        "formant warp is supported: {:?}",
        c.diagnostics
    );
}

#[test]
fn unknown_aliases_and_overrides_are_reported() {
    let mut p = project();
    p.tracks[0].parts[0].notes[1].lyric = "xyz".into();
    p.tracks[0].parts[0].notes[2].phonemes = Some(vec![PhonemeOverride {
        symbol: "n".into(),
        alphabet: PhonemeAlphabet::Native,
        offset_ms: None,
        length_ms: None,
    }]);
    let out = Renderer::new(utau(), cache("diag"))
        .render(&p, Quality::Preview)
        .unwrap();
    let codes: Vec<&str> = out.diagnostics.iter().map(|d| d.code.as_str()).collect();
    assert!(codes.contains(&"phon.unknown_lyric"), "{codes:?}");
    assert!(codes.contains(&"utau.phonemes_unsupported"), "{codes:?}");
}
