//! Builds the phonemizer template and a hostile probe plugin with the
//! current toolchain, then loads them through the host.

use cantor_phon::PhoneKind;
use cantor_phon::wasm::{PluginNote, PluginSet, WasmPhonemizer};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Builds a plugin crate for wasm32-wasip2 and returns the component path.
fn build(manifest: &Path, artifact: &str) -> PathBuf {
    let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("plugins");
    let status = Command::new(env!("CARGO"))
        .args(["build", "--release", "--quiet", "--target", "wasm32-wasip2"])
        .arg("--manifest-path")
        .arg(manifest)
        .arg("--target-dir")
        .arg(&target)
        .status()
        .expect("run cargo");
    assert!(status.success(), "building {}", manifest.display());
    target.join("wasm32-wasip2/release").join(artifact)
}

fn template() -> &'static WasmPhonemizer {
    static P: OnceLock<WasmPhonemizer> = OnceLock::new();
    P.get_or_init(|| {
        let wasm = build(
            &root().join("templates/phonemizer/Cargo.toml"),
            "cantor_phonemizer_ja_kana.wasm",
        );
        WasmPhonemizer::load(&wasm).expect("load template plugin")
    })
}

fn probe() -> &'static WasmPhonemizer {
    static P: OnceLock<WasmPhonemizer> = OnceLock::new();
    P.get_or_init(|| {
        let wasm = build(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/sandbox-plugin/Cargo.toml"),
            "sandbox_probe.wasm",
        );
        WasmPhonemizer::load(&wasm).expect("load probe plugin")
    })
}

fn note(lyric: &str, lang: &str) -> PluginNote {
    PluginNote {
        lyric: lyric.into(),
        lang: lang.into(),
        start: 0.0,
        end: 0.5,
        pitch_cents: 6000.0,
    }
}

#[test]
fn template_phonemizes_kana_and_declares_its_inventory() {
    let p = template();
    assert_eq!(p.languages(), ["ja"]);
    assert!(p.handles("ja-JP") && !p.handles("en"));
    let out = p
        .phonemize(&[
            note("きゃ", "ja"),
            note("+", "ja"),
            note("ん", "ja"),
            note("xq", "ja"),
        ])
        .unwrap();
    assert_eq!(out[0].phonemes, ["ky", "a"]);
    assert!(out[1].phonemes.is_empty());
    assert_eq!(out[2].phonemes, ["N"]);
    assert!(out[3].phonemes.is_empty() && !out[3].warnings.is_empty());

    let inv = p.inventory("ja").unwrap();
    assert_eq!(inv.kind("a"), PhoneKind::Vowel);
    assert_eq!(inv.kind("N"), PhoneKind::Vowel);
    assert_eq!(inv.kind("ky"), PhoneKind::Consonant);
    assert_eq!(inv.xsampa().to_native("k_j"), Some("ky"));
    assert!(p.fingerprint().starts_with("Japanese kana/romaji@0.1.0#"));

    // Plugins are pure: the same input gives the same output.
    let again = p.phonemize(&[note("きゃ", "ja")]).unwrap();
    assert_eq!(again[0], out[0]);
}

#[test]
fn sandbox_hides_the_host_and_freezes_time() {
    let p = probe();
    let out = p
        .phonemize(&[note("fs", "xx"), note("env", "xx"), note("time", "xx")])
        .unwrap();
    assert!(out[0].warnings[0].starts_with("denied"), "{:?}", out[0]);
    assert_eq!(out[1].warnings[0], "0 vars");
    assert_eq!(out[2].warnings[0], "0ns");
}

#[test]
fn runaway_plugins_fail_instead_of_hanging() {
    let p = probe();
    let e = p.phonemize(&[note("loop", "xx")]).unwrap_err();
    assert!(e.to_string().contains("fuel"), "{e}");
    let e = p.phonemize(&[note("alloc", "xx")]).unwrap_err();
    assert!(!e.to_string().is_empty());
    // A trap poisons the instance; the host replaces it, so the plugin
    // keeps working.
    let after = p.phonemize(&[note("env", "xx")]).unwrap();
    assert_eq!(after[0].warnings[0], "0 vars");
}

#[test]
fn plugin_sets_pick_by_language_and_report_bad_files() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("plugin-set");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(template().path(), dir.join("ja.wasm")).unwrap();
    std::fs::write(dir.join("broken.wasm"), b"not wasm").unwrap();
    let (set, errors) = PluginSet::scan(&[dir]);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(set.for_lang("ja").is_some() && set.for_lang("zh").is_none());
    assert!(!set.fingerprint().is_empty());
}
