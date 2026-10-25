//! A WASM phonemizer plugin (the template, built here) replaces the
//! voicebank dictionary for its language, end to end through rendering.

use cantor_backends::DiffSinger;
use cantor_core::*;
use cantor_phon::wasm::{PluginSet, WasmPhonemizer};
use cantor_render::cache::DiskCache;
use cantor_render::render::{Quality, Renderer};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

fn template_plugin() -> Arc<WasmPhonemizer> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("plugins");
    let status = Command::new(env!("CARGO"))
        .args(["build", "--release", "--quiet", "--target", "wasm32-wasip2"])
        .arg("--manifest-path")
        .arg(root.join("templates/phonemizer/Cargo.toml"))
        .arg("--target-dir")
        .arg(&target)
        .status()
        .expect("run cargo");
    assert!(status.success());
    let wasm = target.join("wasm32-wasip2/release/cantor_phonemizer_ja_kana.wasm");
    Arc::new(WasmPhonemizer::load(&wasm).expect("load plugin"))
}

fn voicebank() -> DiffSinger {
    let vb = cantor_fixtures::ensure(&cantor_fixtures::QIXUAN).expect("voicebank fixture");
    let voc = cantor_fixtures::ensure(&cantor_fixtures::PC_NSF_HIFIGAN).expect("vocoder fixture");
    DiffSinger::load(&vb, &voc).expect("load voicebank")
}

fn project(lyrics: &[&str]) -> Project {
    let notes = lyrics
        .iter()
        .enumerate()
        .map(|(i, lyric)| Note {
            id: NoteId(10 + i as u64),
            start: i as i64 * 1920,
            length: 1920,
            pitch_cents: 6700.0 + 200.0 * i as f64,
            lyric: (*lyric).into(),
            lang: Some(LangTag("ja".into())),
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
            id: "qixuan".into(),
            version: None,
        },
        backend: BackendRef("diffsinger".into()),
        mix: Mix::default(),
        effects: vec![],
        doubling: Doubling::default(),
        voice_mix: vec![],
        curves: BTreeMap::new(),
        parts: vec![Part {
            id: PartId(2),
            name: "A".into(),
            start: 0,
            length: 4 * 1920,
            notes,
        }],
    });
    p
}

fn cache(name: &str) -> DiskCache {
    let d = std::env::temp_dir().join(format!("cantor-plug-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    DiskCache::new(d).unwrap()
}

#[test]
fn plugin_phonemizes_kana_that_the_dictionary_cannot() {
    let kana = project(&["か", "さ"]);
    let plain = Renderer::new(Arc::new(voicebank()), cache("plain"));
    let without = plain.render(&kana, Quality::Preview).unwrap();
    assert!(
        without
            .diagnostics
            .iter()
            .any(|d| d.code == "phon.unknown_lyric"),
        "the dictionary is keyed by romaji: {:?}",
        without.diagnostics
    );
    let romaji = plain
        .render(&project(&["ka", "sa"]), Quality::Preview)
        .unwrap();
    assert!(romaji.diagnostics.is_empty(), "{:?}", romaji.diagnostics);

    let set = Arc::new(PluginSet::new(vec![template_plugin()]));
    let ds = voicebank().with_phonemizers(set);
    assert!(
        cantor_render::backend::Backend::version(&ds).contains("Japanese kana/romaji@0.1.0"),
        "plugin identity is part of the cache key"
    );
    assert_eq!(ds.phonemes_for("きゃ", Some("ja")).unwrap(), ["ky", "a"]);
    assert_eq!(ds.xsampa("ja").unwrap().to_native("k_j"), Some("ky"));
    let with = Renderer::new(Arc::new(ds), cache("plugin"))
        .render(&kana, Quality::Preview)
        .unwrap();
    assert!(with.diagnostics.is_empty(), "{:?}", with.diagnostics);
    // Same phonemes as the dictionary's romaji entries: identical audio.
    assert_eq!(with.audio_hash, romaji.audio_hash);
}
