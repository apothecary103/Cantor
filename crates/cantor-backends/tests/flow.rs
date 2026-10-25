//! The flow-matching adapter with a real (tiny, hand-built) ONNX velocity
//! model `v = cond - x`, wrapping a synthetic backend.

use cantor_backends::flow::{FlowBackend, Solver, initial_state, integrate};
use cantor_backends::onnx_noise::test_support::velocity_model;
use cantor_core::*;
use cantor_render::backend::*;
use cantor_render::cache::DiskCache;
use cantor_render::render::{Quality, Renderer};
use cantor_render::rng::Rng;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Minimal backend: one phone, mel = seeded noise shaped by f0.
struct Synth;

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
        &["pitch"]
    }
    fn param_default(&self, _: &str) -> f32 {
        0.0
    }
    fn param_range(&self, _: &str) -> (f32, f32) {
        (-1.0, 1.0)
    }
    fn predict(&self, input: &PhraseInput<'_>) -> Result<Prediction, BackendError> {
        Ok(Prediction {
            phones: vec![Phone {
                symbol: "a".into(),
                frames: input.frames as i64,
            }],
            pitch: None,
            variance: BTreeMap::new(),
            diagnostics: vec![],
        })
    }
    fn acoustic(&self, input: &AcousticInput<'_>) -> Result<Vec<f32>, BackendError> {
        let mut rng = Rng::new(input.seed);
        Ok(input
            .f0_ref_hz
            .iter()
            .flat_map(|f| {
                rng.normals_f32(4)
                    .into_iter()
                    .map(move |n| n * 0.1 + f / 500.0)
            })
            .collect())
    }
    fn vocode(&self, mel: &[f32], f0_hz: &[f32]) -> Result<Vec<f32>, BackendError> {
        let mut phase = 0.0f64;
        let mut out = Vec::new();
        for (i, f) in f0_hz.iter().enumerate() {
            let amp: f32 = mel[i * 4..i * 4 + 4].iter().sum::<f32>() / 8.0;
            for _ in 0..80 {
                phase += std::f64::consts::TAU * f64::from(*f) / 8000.0;
                out.push(amp * phase.sin() as f32);
            }
        }
        Ok(out)
    }
}

fn model_dir(name: &str, depth: f32, extra: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("flow-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("velocity.onnx"), velocity_model()).unwrap();
    std::fs::write(
        dir.join("flow.yaml"),
        format!("velocity: velocity.onnx\ndepth: {depth}\nsolver: heun\nversion: test\n{extra}"),
    )
    .unwrap();
    dir.join("flow.yaml")
}

fn input<'a>(
    f0: &'a [f32],
    variance: &'a BTreeMap<String, Vec<f32>>,
    steps: u32,
) -> AcousticInput<'a> {
    AcousticInput {
        phones: &[],
        f0_ref_hz: f0,
        variance,
        steps,
        seed: 42,
    }
}

#[test]
fn the_onnx_velocity_field_drives_the_ode() {
    let f0 = vec![220.0f32; 25];
    let variance = BTreeMap::new();
    let prior = Synth.acoustic(&input(&f0, &variance, 10)).unwrap();

    // depth 0: the prior passes through untouched.
    let shallow0 = FlowBackend::load(Arc::new(Synth), &model_dir("d0", 0.0, "")).unwrap();
    assert_eq!(
        shallow0.acoustic(&input(&f0, &variance, 10)).unwrap(),
        prior
    );

    // depth 0.6: identical to integrating v = cond - x in Rust, for the
    // preview (Euler) and final (Heun) solvers.
    let flow = FlowBackend::load(Arc::new(Synth), &model_dir("d6", 0.6, "")).unwrap();
    for (steps, solver) in [(10, Solver::Euler), (20, Solver::Heun)] {
        let got = flow.acoustic(&input(&f0, &variance, steps)).unwrap();
        let mut v = |x: &[f32], _t: f32| -> Result<Vec<f32>, BackendError> {
            Ok(prior.iter().zip(x).map(|(c, xi)| c - xi).collect())
        };
        let want = integrate(&mut v, initial_state(&prior, 0.6, 42), 0.4, steps, solver).unwrap();
        for (g, w) in got.iter().zip(&want) {
            assert!((g - w).abs() < 1e-5, "{steps} steps: {g} vs {w}");
        }
    }
    assert!(flow.version().contains("flow:test:0.6:Heun"));
}

#[test]
fn renders_deterministically_through_the_renderer() {
    let flow = Arc::new(FlowBackend::load(Arc::new(Synth), &model_dir("render", 0.5, "")).unwrap());
    let mut p = Project::new();
    p.tracks.push(Track {
        id: TrackId(1),
        name: "t".into(),
        singer: SingerRef::default(),
        backend: BackendRef("flow".into()),
        mix: Mix::default(),
        effects: vec![],
        doubling: Doubling::default(),
        voice_mix: vec![],
        curves: BTreeMap::new(),
        parts: vec![Part {
            id: PartId(2),
            name: "p".into(),
            start: 0,
            length: 3840,
            notes: vec![Note {
                id: NoteId(3),
                start: 0,
                length: 1920,
                pitch_cents: 6000.0,
                lyric: "la".into(),
                lang: None,
                phonemes: None,
                expr: NoteExpr::default(),
                modes: VocalModes::default(),
            }],
        }],
    });
    let cache = |n: &str| {
        let d = std::env::temp_dir().join(format!("cantor-flow-{n}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        DiskCache::new(d).unwrap()
    };
    let a = Renderer::new(flow.clone(), cache("a"))
        .render(&p, Quality::Preview)
        .unwrap();
    let b = Renderer::new(flow.clone(), cache("b"))
        .render(&p, Quality::Preview)
        .unwrap();
    assert_eq!(a.audio_hash, b.audio_hash);
    let c = Renderer::new(flow, cache("c"))
        .render(&p, Quality::Final)
        .unwrap();
    assert_ne!(a.audio_hash, c.audio_hash, "steps are the quality knob");
}

#[test]
fn mismatched_tensor_names_are_reported() {
    let path = model_dir("names", 1.0, "names:\n  x: latent\n");
    let e = FlowBackend::load(Arc::new(Synth), &path).unwrap_err();
    assert!(e.0.contains("no input `latent`"), "{e}");
}
