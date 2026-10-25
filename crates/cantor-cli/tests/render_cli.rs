//! M2 done-when: `cantor render` produces a WAV from a public DiffSinger
//! voicebank, with ONNX Runtime loaded from ORT_DYLIB_PATH.

use std::path::Path;
use std::process::Command;

const PROJECT: &str = include_str!("fixtures/twinkle.cantor");

fn cantor() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cantor"))
}

#[test]
fn render_writes_a_wav() {
    assert!(
        std::env::var_os("ORT_DYLIB_PATH").is_some(),
        "run inside the Nix dev shell: ONNX Runtime comes from ORT_DYLIB_PATH"
    );
    let vb = cantor_fixtures::ensure(&cantor_fixtures::QIXUAN).unwrap();
    let voc = cantor_fixtures::ensure(&cantor_fixtures::PC_NSF_HIFIGAN).unwrap();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("render-cli");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let project = dir.join("twinkle.cantor");
    std::fs::write(&project, PROJECT).unwrap();
    let wav = dir.join("twinkle.wav");

    let run = |cache: &str| {
        let out = cantor()
            .args(["render", "--quality", "preview", "--json", "-o"])
            .arg(&wav)
            .arg("--voicebank")
            .arg(&vb)
            .arg("--vocoder")
            .arg(&voc)
            .arg("--cache")
            .arg(dir.join(cache))
            .arg(&project)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()
    };
    let first = run("cache");
    let bytes = std::fs::read(&wav).unwrap();
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    assert!(first["seconds"].as_f64().unwrap() > 3.0);

    let again = run("cache");
    assert_eq!(first["audio_hash"], again["audio_hash"]);
    let traces = again["trace"]["phrases"].as_array().unwrap();
    assert!(traces.iter().all(|t| t["outcome"] == "cache_hit"));
}

#[test]
fn ops_apply_edits_a_project() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("ops-cli");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let project = dir.join("song.cantor");
    std::fs::write(&project, PROJECT).unwrap();
    let ops = dir.join("edits.json");
    std::fs::write(&ops, r#"[{"op": "set_lyric", "id": 100, "lyric": "la"}]"#).unwrap();

    let dry = cantor()
        .args(["ops", "apply", "--dry-run"])
        .arg(&project)
        .arg(&ops)
        .output()
        .unwrap();
    assert!(dry.status.success());
    assert_eq!(std::fs::read_to_string(&project).unwrap(), PROJECT);

    let real = cantor()
        .args(["ops", "apply"])
        .arg(&project)
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        real.status.success(),
        "{}",
        String::from_utf8_lossy(&real.stderr)
    );
    assert!(
        std::fs::read_to_string(&project)
            .unwrap()
            .contains("\"lyric\": \"la\"")
    );

    std::fs::write(&ops, r#"{"op": "set_lyric", "id": 999, "lyric": "x"}"#).unwrap();
    let bad = cantor()
        .args(["ops", "apply"])
        .arg(&project)
        .arg(&ops)
        .output()
        .unwrap();
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stdout).contains("note.not_found"));
}

/// Without `--voicebank`, `render` and `analyze` go through the engine:
/// any backend (here UTAU, with a generated voicebank) from $CANTOR_SINGERS.
#[test]
fn engine_render_and_analyze_with_a_utau_singer() {
    use cantor_core::*;
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("engine-cli");
    let _ = std::fs::remove_dir_all(&dir);
    let singers = dir.join("singers");
    cantor_fixtures::utau::write_synthetic_voicebank(&singers.join("synthetic-cv")).unwrap();
    let rmvpe = cantor_fixtures::ensure(&cantor_fixtures::RMVPE).unwrap();

    let mut p = Project::new();
    let notes = ["ka", "sa", "na"]
        .iter()
        .enumerate()
        .map(|(i, l)| Note {
            id: NoteId(10 + i as u64),
            start: i as i64 * 1920,
            length: 1920,
            pitch_cents: 6000.0 + 200.0 * i as f64,
            lyric: (*l).into(),
            lang: None,
            phonemes: None,
            expr: NoteExpr::default(),
            modes: VocalModes::default(),
        })
        .collect();
    p.tracks.push(Track {
        id: TrackId(1),
        name: "Lead".into(),
        singer: SingerRef {
            id: "synthetic-cv".into(),
            version: None,
        },
        backend: BackendRef("utau".into()),
        mix: Mix::default(),
        effects: vec![EffectSlot::new(EffectKind::Reverb)],
        doubling: Doubling::default(),
        voice_mix: vec![],
        curves: Default::default(),
        parts: vec![Part {
            id: PartId(2),
            name: "A".into(),
            start: 0,
            length: 4 * 1920,
            notes,
        }],
    });
    let project = dir.join("song.cantor");
    cantor_io::project_file::save(&project, &p, cantor_io::project_file::Compression::None)
        .unwrap();
    let env = |c: &mut Command| {
        c.env("CANTOR_SINGERS", &singers)
            .env("CANTOR_RMVPE", &rmvpe)
            .env("CANTOR_CACHE", dir.join("cache"));
    };

    let wav = dir.join("song.wav");
    let mut render = cantor();
    env(&mut render);
    let out = render
        .args(["render", "--quality", "preview", "--json", "-o"])
        .arg(&wav)
        .arg(&project)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["diagnostics"], serde_json::json!([]));
    assert!(std::fs::metadata(&wav).unwrap().len() > 44_100);

    let mut analyze = cantor();
    env(&mut analyze);
    let out = analyze.arg("analyze").arg(&project).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("p95") && text.contains("LUFS"), "{text}");
}
