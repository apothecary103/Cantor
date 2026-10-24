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
