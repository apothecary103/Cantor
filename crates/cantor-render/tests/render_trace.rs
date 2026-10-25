//! Render-pipeline contract tests with a synthetic backend: determinism,
//! caching and the vocoder fast path. The same properties are checked with
//! a real DiffSinger voicebank in cantor-backends' tests.

use cantor_core::*;
use cantor_render::backend::*;
use cantor_render::cache::DiskCache;
use cantor_render::render::{Outcome, Quality, Renderer};
use cantor_render::rng::Rng;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Phones: one per note frame span; mel: noise shaped by f0; audio: a sine.
#[derive(Default)]
struct Synth {
    acoustic_runs: AtomicUsize,
    vocoder_runs: AtomicUsize,
}

impl Backend for Synth {
    fn id(&self) -> &str {
        "synth"
    }
    fn version(&self) -> &str {
        "1"
    }
    fn grid(&self) -> Grid {
        Grid {
            sample_rate: 8000,
            hop: 80,
            mel_bins: 4,
        }
    }
    fn params(&self) -> &[&'static str] {
        &["pitch", "breathiness"]
    }
    fn param_default(&self, _: &str) -> f32 {
        -20.0
    }
    fn param_range(&self, _: &str) -> (f32, f32) {
        (-96.0, 0.0)
    }
    fn predict(&self, input: &PhraseInput<'_>) -> Result<Prediction, BackendError> {
        Ok(Prediction {
            phones: vec![Phone {
                symbol: "a".into(),
                frames: input.frames as i64,
            }],
            pitch: Some(input.base_pitch.iter().map(|&c| c as f32 + 3.0).collect()),
            variance: BTreeMap::from([("breathiness".to_owned(), vec![-30.0; input.frames])]),
            diagnostics: vec![],
        })
    }
    fn acoustic(&self, input: &AcousticInput<'_>) -> Result<Vec<f32>, BackendError> {
        self.acoustic_runs.fetch_add(1, Ordering::SeqCst);
        let mut rng = Rng::new(input.seed);
        let bins = self.grid().mel_bins;
        Ok(input
            .f0_ref_hz
            .iter()
            .flat_map(|f| {
                let n: Vec<f32> = rng.normals_f32(bins);
                n.into_iter().map(move |x| x * 0.01 + f / 1000.0)
            })
            .collect())
    }
    fn vocode(&self, mel: &[f32], f0_hz: &[f32]) -> Result<Vec<f32>, BackendError> {
        self.vocoder_runs.fetch_add(1, Ordering::SeqCst);
        let g = self.grid();
        let mut phase = 0.0f64;
        let mut out = Vec::with_capacity(f0_hz.len() * g.hop);
        for (i, f) in f0_hz.iter().enumerate() {
            let amp = mel[i * g.mel_bins..(i + 1) * g.mel_bins]
                .iter()
                .sum::<f32>()
                / 10.0;
            for _ in 0..g.hop {
                phase += std::f64::consts::TAU * f64::from(*f) / f64::from(g.sample_rate);
                out.push(amp * phase.sin() as f32);
            }
        }
        Ok(out)
    }
}

fn project() -> Project {
    let mut doc = Document::new(Project::new());
    let notes: Vec<Note> = (0..6)
        .map(|i| Note {
            id: NoteId(10 + i),
            // Two phrases: notes 0–2, a one-beat rest, notes 3–5.
            start: i as i64 * 1920 + if i >= 3 { 1920 } else { 0 },
            length: 1920,
            pitch_cents: 6000.0 + 100.0 * i as f64,
            lyric: "la".into(),
            lang: None,
            phonemes: None,
            expr: NoteExpr::default(),
            modes: VocalModes::default(),
        })
        .collect();
    let r = doc.apply(Op::AddTrack {
        index: None,
        track: Track {
            id: TrackId(1),
            name: "Lead".into(),
            singer: SingerRef::default(),
            backend: BackendRef("synth".into()),
            mix: Mix::default(),
            effects: vec![],
            doubling: Doubling::default(),
            voice_mix: vec![],
            curves: BTreeMap::new(),
            parts: vec![Part {
                id: PartId(2),
                name: "Verse".into(),
                start: 0,
                length: 7 * 1920,
                notes,
            }],
        },
    });
    assert!(r.applied, "{:?}", r.diagnostics);
    doc.project().clone()
}

fn renderer(dir: &std::path::Path) -> (Renderer, Arc<Synth>) {
    let synth = Arc::new(Synth::default());
    let r = Renderer::new(synth.clone(), DiskCache::new(dir).unwrap());
    (r, synth)
}

fn tempdir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cantor-render-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn same_project_same_hash_and_full_cache_hits() {
    let p = project();
    let dir = tempdir("hash");
    let (r, synth) = renderer(&dir);
    let first = r.render(&p, Quality::Preview).unwrap();
    assert_eq!(first.trace.phrases.len(), 2);
    assert!(
        first
            .trace
            .phrases
            .iter()
            .all(|t| t.outcome == Outcome::Full)
    );
    let second = r.render(&p, Quality::Preview).unwrap();
    assert_eq!(first.audio_hash, second.audio_hash);
    assert_eq!(second.trace.cache_hit_rate(), 1.0);
    assert_eq!(synth.acoustic_runs.load(Ordering::SeqCst), 2);

    // A cold cache gives the same audio: rendering is a pure function.
    let (cold, _) = renderer(&tempdir("hash-cold"));
    assert_eq!(
        cold.render(&p, Quality::Preview).unwrap().audio_hash,
        first.audio_hash
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn pitch_only_edit_takes_the_vocoder_fast_path() {
    let p = project();
    let dir = tempdir("fast");
    let (r, synth) = renderer(&dir);
    let before = r.render(&p, Quality::Preview).unwrap();

    // Bend pitch inside the second phrase only.
    let mut doc = Document::new(p);
    let applied = doc.apply(Op::SetCurve {
        track: TrackId(1),
        param: ParamId::new("pitch"),
        range: TickRange {
            start: 4 * 1920,
            end: 6 * 1920,
        },
        points: vec![(4 * 1920, 0.0), (5 * 1920, 80.0), (6 * 1920 - 1, 0.0)],
        interp: Interp::MonotoneHermite,
    });
    assert!(applied.applied);
    let acoustic_before = synth.acoustic_runs.load(Ordering::SeqCst);
    let after = r.render(doc.project(), Quality::Preview).unwrap();

    let outcomes: Vec<_> = after.trace.phrases.iter().map(|t| t.outcome).collect();
    assert_eq!(outcomes, [Outcome::CacheHit, Outcome::VocoderFastPath]);
    assert_eq!(synth.acoustic_runs.load(Ordering::SeqCst), acoustic_before);
    assert_ne!(before.audio_hash, after.audio_hash);

    // The fast path gives exactly what a cold render gives.
    let (cold, _) = renderer(&tempdir("fast-cold"));
    assert_eq!(
        cold.render(doc.project(), Quality::Preview)
            .unwrap()
            .audio_hash,
        after.audio_hash
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn non_pitch_edit_rerenders_the_acoustic_model() {
    let p = project();
    let dir = tempdir("variance");
    let (r, _) = renderer(&dir);
    r.render(&p, Quality::Preview).unwrap();
    let mut doc = Document::new(p);
    doc.apply(Op::SetCurve {
        track: TrackId(1),
        param: ParamId::new("breathiness"),
        range: TickRange {
            start: 0,
            end: 1920,
        },
        points: vec![(0, 6.0), (1900, 6.0)],
        interp: Interp::Linear,
    });
    let after = r.render(doc.project(), Quality::Preview).unwrap();
    assert_eq!(after.trace.phrases[0].outcome, Outcome::Full);
    assert_eq!(after.trace.phrases[1].outcome, Outcome::CacheHit);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unsupported_curves_are_reported() {
    let p = project();
    let mut doc = Document::new(p);
    doc.apply(Op::SetCurve {
        track: TrackId(1),
        param: ParamId::new("tension"),
        range: TickRange { start: 0, end: 100 },
        points: vec![(0, 1.0)],
        interp: Interp::Linear,
    });
    let (r, _) = renderer(&tempdir("unsupported"));
    let out = r.render(doc.project(), Quality::Preview).unwrap();
    assert!(
        out.diagnostics
            .iter()
            .any(|d| d.code == "render.param_unsupported")
    );
}

#[test]
fn retakes_change_only_their_phrase_and_switch_back_exactly() {
    let p = project();
    let (r, _) = renderer(&tempdir("retake"));
    let original = r.render(&p, Quality::Preview).unwrap();
    let mut doc = Document::new(p);
    let res = doc.apply(Op::Retake {
        track: TrackId(1),
        range: TickRange {
            start: 4 * 1920,
            end: 5 * 1920,
        },
        dims: vec![RetakeDim::Timbre],
        n_variants: 2,
        seed: Some(5),
    });
    assert!(res.applied, "{:?}", res.diagnostics);
    let retaken = r.render(doc.project(), Quality::Preview).unwrap();
    let outcomes: Vec<_> = retaken.trace.phrases.iter().map(|t| t.outcome).collect();
    assert_eq!(outcomes, [Outcome::CacheHit, Outcome::Full]);
    assert_ne!(retaken.audio_hash, original.audio_hash);

    let phrase = retaken.trace.phrases[1].phrase;
    doc.apply(Op::SelectTake { phrase, take: None });
    let back = r.render(doc.project(), Quality::Preview).unwrap();
    assert_eq!(back.audio_hash, original.audio_hash);
    assert_eq!(back.trace.cache_hit_rate(), 1.0);
}

fn set_track(p: &Project, patch: TrackPatch) -> Project {
    let mut doc = Document::new(p.clone());
    let r = doc.apply(Op::SetTrack {
        id: TrackId(1),
        patch,
    });
    assert!(r.applied, "{:?}", r.diagnostics);
    doc.project().clone()
}

#[test]
fn doubling_revocodes_the_cached_mel_with_seeded_layers() {
    let dry = project();
    let doubled = set_track(
        &dry,
        TrackPatch {
            doubling: Some(Doubling {
                layers: 4,
                spacing: 0.8,
                ..Doubling::default()
            }),
            ..TrackPatch::default()
        },
    );
    let dir = tempdir("double");
    let (r, synth) = renderer(&dir);
    let a = r.render(&dry, Quality::Preview).unwrap();
    let (acoustic, vocoder) = (
        synth.acoustic_runs.load(Ordering::SeqCst),
        synth.vocoder_runs.load(Ordering::SeqCst),
    );
    let b = r.render(&doubled, Quality::Preview).unwrap();
    assert!(b.diagnostics.is_empty(), "{:?}", b.diagnostics);
    // Three doubles per phrase, all from the cached mel: no acoustic runs.
    assert_eq!(synth.acoustic_runs.load(Ordering::SeqCst), acoustic);
    assert_eq!(synth.vocoder_runs.load(Ordering::SeqCst), vocoder + 6);
    assert_ne!(a.audio_hash, b.audio_hash);
    // The doubles are spread: left and right differ.
    let (l, r_ch): (Vec<f32>, Vec<f32>) = b.samples.chunks(2).map(|f| (f[0], f[1])).unzip();
    assert_ne!(l, r_ch);
    // Fully cached on a second render, and a cold cache gives the same audio.
    let again = r.render(&doubled, Quality::Preview).unwrap();
    assert_eq!(again.audio_hash, b.audio_hash);
    assert_eq!(synth.vocoder_runs.load(Ordering::SeqCst), vocoder + 6);
    let (cold, _) = renderer(&tempdir("double-cold"));
    assert_eq!(
        cold.render(&doubled, Quality::Preview).unwrap().audio_hash,
        b.audio_hash
    );
    // Turning doubling off restores the dry render exactly.
    let off = set_track(
        &doubled,
        TrackPatch {
            doubling: Some(Doubling::default()),
            ..TrackPatch::default()
        },
    );
    assert_eq!(
        r.render(&off, Quality::Preview).unwrap().audio_hash,
        a.audio_hash
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn effects_chain_is_rendered_and_bypass_is_exact() {
    let dry = project();
    let mut reverb = EffectSlot::new(EffectKind::Reverb);
    reverb.params.insert("mix".into(), 0.4);
    let comp = EffectSlot::new(EffectKind::Compressor);
    let wet = set_track(
        &dry,
        TrackPatch {
            effects: Some(vec![comp.clone(), reverb.clone()]),
            ..TrackPatch::default()
        },
    );
    let dir = tempdir("fx");
    let (r, _) = renderer(&dir);
    let a = r.render(&dry, Quality::Preview).unwrap();
    let b = r.render(&wet, Quality::Preview).unwrap();
    assert!(b.diagnostics.is_empty(), "{:?}", b.diagnostics);
    assert_ne!(a.audio_hash, b.audio_hash);
    let tail = cantor_render::fx::tail_secs(&reverb);
    let extra = (b.samples.len() - a.samples.len()) / 2;
    assert_eq!(
        extra,
        (tail * 8000.0).ceil() as usize,
        "reverb tail appended"
    );
    assert_eq!(
        r.render(&wet, Quality::Preview).unwrap().audio_hash,
        b.audio_hash
    );

    // Disabled slots leave the render bit-identical to the dry one.
    let bypassed = set_track(
        &dry,
        TrackPatch {
            effects: Some(
                [comp, reverb]
                    .into_iter()
                    .map(|mut s| {
                        s.enabled = false;
                        s
                    })
                    .collect(),
            ),
            ..TrackPatch::default()
        },
    );
    assert_eq!(
        r.render(&bypassed, Quality::Preview).unwrap().audio_hash,
        a.audio_hash
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn invalid_effect_parameters_are_rejected_with_a_diagnostic() {
    let mut doc = Document::new(project());
    let mut slot = EffectSlot::new(EffectKind::Eq);
    slot.params.insert("mid_gain_db".into(), 60.0);
    slot.params.insert("loudness".into(), 1.0);
    let r = doc.apply(Op::SetTrack {
        id: TrackId(1),
        patch: TrackPatch {
            effects: Some(vec![slot]),
            ..TrackPatch::default()
        },
    });
    assert!(!r.applied);
    let text = format!("{:?}", r.diagnostics);
    assert!(
        text.contains("loudness") && text.contains("mid_gain_db"),
        "{text}"
    );
}
