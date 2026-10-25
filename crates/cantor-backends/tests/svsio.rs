//! The svs.io adapter against a real ZeroMQ backend: the Python reference
//! backend in tests/svsio (pyzmq + msgpack from the Nix shell).

use cantor_backends::rmvpe::Rmvpe;
use cantor_backends::svsio::{SvsIo, SvsIoConfig};
use cantor_core::*;
use cantor_render::cache::DiskCache;
use cantor_render::render::{Outcome, Quality, Renderer};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

/// The reference backend, killed when dropped.
struct Server(Child, String);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn server() -> Server {
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/svsio/reference_backend.py");
    let mut child = Command::new("python3")
        .arg(script)
        .stdout(Stdio::piped())
        .spawn()
        .expect("python3 with pyzmq and msgpack (the Nix dev shell provides them)");
    let mut line = String::new();
    BufReader::new(child.stdout.as_mut().unwrap())
        .read_line(&mut line)
        .unwrap();
    let port: u16 = line.trim().parse().expect("the backend prints its port");
    Server(child, format!("tcp://127.0.0.1:{port}"))
}

fn project() -> Project {
    let melody = [
        (0, 62.0, "ka"),
        (1, 64.0, "sa"),
        (2, 66.0, "na"),
        (3, 67.0, "o"),
    ];
    let notes = melody
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
        name: "Lead".into(),
        singer: SingerRef {
            id: "reference".into(),
            version: None,
        },
        backend: BackendRef("svsio".into()),
        mix: Mix::default(),
        effects: vec![],
        doubling: Doubling::default(),
        voice_mix: vec![],
        curves: BTreeMap::new(),
        parts: vec![Part {
            id: PartId(2),
            name: "A".into(),
            start: 0,
            length: 5 * 1920,
            notes,
        }],
    });
    p
}

fn cache(name: &str) -> DiskCache {
    let d = std::env::temp_dir().join(format!("cantor-svsio-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    DiskCache::new(d).unwrap()
}

fn rmvpe() -> Arc<Rmvpe> {
    let p = cantor_fixtures::ensure(&cantor_fixtures::RMVPE).expect("RMVPE fixture");
    Arc::new(Rmvpe::load(&p).expect("load RMVPE"))
}

#[test]
fn world_feature_backend_sings_in_tune_with_a_local_fast_path() {
    let s = server();
    let backend = Arc::new(
        SvsIo::connect(
            "reference",
            &SvsIoConfig {
                endpoint: s.1.clone(),
                prefer_audio: false,
                languages: vec![],
            },
        )
        .unwrap(),
    );
    assert!(backend.uses_world_features());
    assert_eq!(backend.ops().len(), 3);
    let params = cantor_render::backend::Backend::params(backend.as_ref());
    assert!(params.contains(&"breathiness") && !params.contains(&"tension"));

    let r = Renderer::new(backend.clone(), cache("world")).with_analyzer(rmvpe());
    let p = project();
    let a = r.render(&p, Quality::Preview).unwrap();
    assert!(a.diagnostics.is_empty(), "{:?}", a.diagnostics);
    let report = a.analysis.as_ref().unwrap();
    let p95 = report.tracks[0].pitch.p95_cents.expect("measured");
    assert!(p95 < 20.0, "p95 pitch error {p95:.1} cents");

    // Deterministic, and a pitch edit re-synthesizes locally.
    let cold = Renderer::new(backend.clone(), cache("world-cold"))
        .render(&p, Quality::Preview)
        .unwrap();
    assert_eq!(cold.audio_hash, a.audio_hash);
    let mut doc = Document::new(p);
    let applied = doc.apply(Op::SetCurve {
        track: TrackId(1),
        param: ParamId::new("pitch"),
        range: TickRange {
            start: 1920,
            end: 3840,
        },
        points: vec![(1920, 50.0), (3839, 50.0)],
        interp: Interp::Linear,
    });
    assert!(applied.applied, "{:?}", applied.diagnostics);
    let b = r.render(doc.project(), Quality::Preview).unwrap();
    assert!(
        b.trace
            .phrases
            .iter()
            .all(|t| t.outcome == Outcome::VocoderFastPath),
        "{:?}",
        b.trace
    );
}

#[test]
fn audio_backend_renders_through_remote_calls() {
    let s = server();
    let backend = Arc::new(
        SvsIo::connect(
            "reference",
            &SvsIoConfig {
                endpoint: s.1.clone(),
                prefer_audio: true,
                languages: vec![],
            },
        )
        .unwrap(),
    );
    assert!(!backend.uses_world_features());
    let r = Renderer::new(backend, cache("audio")).with_analyzer(rmvpe());
    let out = r.render(&project(), Quality::Preview).unwrap();
    assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
    let p95 = out.analysis.as_ref().unwrap().tracks[0]
        .pitch
        .p95_cents
        .expect("measured");
    assert!(p95 < 20.0, "p95 pitch error {p95:.1} cents");
}

#[test]
fn unreachable_backends_fail_quickly() {
    let t = std::time::Instant::now();
    let e = SvsIo::connect(
        "nobody",
        &SvsIoConfig {
            endpoint: "tcp://127.0.0.1:9".into(),
            prefer_audio: false,
            languages: vec![],
        },
    )
    .unwrap_err();
    assert!(t.elapsed().as_secs() < 3, "{e}");
}
