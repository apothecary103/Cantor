//! End-to-end synthesis with the generated CC0 voicebank.

use cantor_core::{Note, NoteId, Project, Track, TrackId};
use cantor_synth::{RenderRequest, Synth, Voicebank};
use std::sync::Arc;

fn bank() -> Arc<Voicebank> {
    // Generated once per test binary (the generator's temp name is per process).
    static BANK: std::sync::OnceLock<Arc<Voicebank>> = std::sync::OnceLock::new();
    BANK.get_or_init(|| {
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("cantor-synthetic");
        let dir = cantor_fixtures::utau::ensure_synthetic_voicebank(&dir).expect("fixture bank");
        Arc::new(Voicebank::load(&dir).expect("loads"))
    })
    .clone()
}

fn project(singer: &str) -> Project {
    let lyrics = [
        ("か", 60),
        ("き", 62),
        ("く", 64),
        ("け", 65),
        ("こ", 67),
        ("あ", 64),
        ("い", 62),
        ("う", 60),
    ];
    let mut notes = Vec::new();
    for (i, (l, k)) in lyrics.iter().enumerate() {
        // Two phrases: a two-beat gap after the fourth note.
        let start = i as i64 * 480 + if i >= 4 { 960 } else { 0 };
        notes.push(Note {
            id: NoteId(i as u64 + 1),
            ..Note::new(start, 480, *k, *l)
        });
    }
    let mut p = Project::default();
    p.tracks.push(Track {
        id: TrackId(100),
        name: "Vocal".into(),
        singer: singer.into(),
        notes,
        ..Track::default()
    });
    p
}

#[test]
fn renders_on_pitch_deterministically_and_caches_phrases() {
    let vb = bank();
    let banks = [vb.clone()];
    let p = project(&vb.id);
    let synth = Synth::new();
    let req = RenderRequest {
        project: &p,
        banks: &banks,
        reference: None,
        tracks: None,
    };
    let a = synth.render(&req);
    assert!(
        a.report.missing_aliases.is_empty(),
        "{:?}",
        a.report.missing_aliases
    );
    assert!(
        a.diagnostics.iter().all(|d| !d.is_error()),
        "{:?}",
        a.diagnostics
    );
    assert_eq!(a.phrases_rendered, 2);
    // The fixture's recordings peak at full scale; crossfades may add a bit.
    let peak = a.samples.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(peak > 0.3 && peak < 1.6, "peak {peak}");
    let err = a
        .report
        .pitch_error
        .clone()
        .expect("voiced frames measured");
    assert!(err.p50 < 15.0 && err.p95 < 60.0, "pitch error {err:?}");
    assert_eq!(a.report.phrases.len(), 2);

    // Same input, fresh synth: bit-identical. Same synth: all from cache.
    let b = Synth::new().render(&req);
    assert_eq!(a.samples, b.samples);
    let c = synth.render(&req);
    assert_eq!((c.phrases_rendered, c.phrases_cached), (0, 2));
    assert_eq!(a.samples, c.samples);

    // Editing a note in the second phrase re-renders only that phrase.
    let mut q = p.clone();
    q.tracks[0].notes[6].key = 65;
    let d = synth.render(&RenderRequest { project: &q, ..req });
    assert_eq!((d.phrases_rendered, d.phrases_cached), (1, 1));
}

#[test]
fn missing_aliases_are_reported() {
    let vb = bank();
    let banks = [vb.clone()];
    let mut p = project(&vb.id);
    p.tracks[0].notes[2].lyric = "xyz".into();
    let r = Synth::new().render(&RenderRequest {
        project: &p,
        banks: &banks,
        reference: None,
        tracks: None,
    });
    assert_eq!(r.report.missing_aliases.len(), 1);
    assert_eq!(r.report.missing_aliases[0].note, NoteId(3));
    assert!(r.diagnostics.iter().any(|d| d.code == "alias.missing"));
}
