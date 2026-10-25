//! OpenUtau USTX import tests.

use cantor_io::import::{ImportError, Imported, ustx};
use cantor_io::project_file::{self, Compression};
use std::path::PathBuf;

/// JSON snapshots with sorted map keys, so they do not depend on whether
/// serde_json's `preserve_order` feature is unified into the build.
macro_rules! assert_snapshot_sorted {
    ($($t:tt)*) => {
        insta::with_settings!({ sort_maps => true }, {
            insta::assert_json_snapshot!($($t)*);
        })
    };
}

fn fixture(name: &str) -> Vec<u8> {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "tests",
        "fixtures",
        "ustx",
        name,
    ]
    .iter()
    .collect();
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn import_fixture(name: &str) -> Imported {
    ustx::import(&fixture(name)).expect("fixture imports")
}

fn snapshot_value(imported: &Imported) -> serde_json::Value {
    serde_json::json!({
        "project": imported.project,
        "diagnostics": imported.diagnostics,
    })
}

fn codes(imported: &Imported) -> Vec<&str> {
    imported
        .diagnostics
        .iter()
        .map(|d| d.code.as_str())
        .collect()
}

#[test]
fn basic_fixture() {
    let imported = import_fixture("basic.ustx");
    assert_snapshot_sorted!("basic", snapshot_value(&imported));
}

#[test]
fn legacy_fixture() {
    let imported = import_fixture("legacy.ustx");
    assert_snapshot_sorted!("legacy", snapshot_value(&imported));
}

#[test]
fn import_is_deterministic() {
    assert_eq!(import_fixture("basic.ustx"), import_fixture("basic.ustx"));
}

#[test]
fn round_trips_through_project_file() {
    for name in ["basic.ustx", "legacy.ustx"] {
        let project = import_fixture(name).project;
        for compression in [Compression::None, Compression::Zstd(3)] {
            let bytes = project_file::to_bytes(&project, compression);
            let loaded = project_file::from_bytes(&bytes).expect("loads");
            assert_eq!(loaded.project, project, "{name}");
            assert!(
                loaded.diagnostics.is_empty(),
                "{name}: {:?}",
                loaded.diagnostics
            );
            assert_eq!(loaded.migrated_from, None);
        }
    }
}

#[test]
fn malformed_yaml_is_a_parse_error() {
    for bad in [
        &b"tracks: [unclosed"[..],
        b"- just\n- a list\n",
        b"\xff\xfe not utf-8",
        b"resolution: 0\ntracks: []\nustx_version: '0.6'\n",
        b"ustx_version: '0.6'\ntracks: [{singer: s}]\nvoice_parts:\n- position: 99999999999999999\n  notes: []\n",
    ] {
        match ustx::import(bad) {
            Err(ImportError::Parse { .. }) => {}
            other => panic!(
                "{:?}: expected a parse error, got {other:?}",
                String::from_utf8_lossy(bad)
            ),
        }
    }
}

#[test]
fn non_ustx_yaml_is_unsupported() {
    assert!(matches!(
        ustx::import(b"foo: bar\n"),
        Err(ImportError::Unsupported { .. })
    ));
}

#[test]
fn inexact_resolution_is_reported() {
    let yaml = br"
ustx_version: '0.9'
resolution: 1000
tracks:
- singer: s
  track_name: T
voice_parts:
- track_no: 0
  position: 0
  duration: 1000
  notes:
  - {position: 1, duration: 999, tone: 60, lyric: a}
";
    let imported = ustx::import(yaml).expect("imports");
    let diag = imported
        .diagnostics
        .iter()
        .find(|d| d.code == "ustx.resolution_inexact")
        .expect("inexact diagnostic");
    assert!(diag.message.contains("1000"), "{}", diag.message);
    let note = &imported.project.tracks[0].parts[0].notes[0];
    // 1 / 1000 quarter notes = 1.92 ticks -> 2; end 1000/1000 = 1920.
    assert_eq!((note.start, note.length), (2, 1918));
}

#[test]
fn exact_resolutions_report_nothing_about_resolution() {
    let imported = import_fixture("basic.ustx");
    assert!(!codes(&imported).contains(&"ustx.resolution_inexact"));
}

#[test]
fn repeated_problems_are_aggregated() {
    let mut yaml = String::from(
        "ustx_version: '0.6'\ntracks:\n- singer: s\nvoice_parts:\n- track_no: 0\n  duration: 1920000\n  notes:\n",
    );
    for i in 0..500 {
        yaml.push_str(&format!(
            "  - {{position: {}, duration: 480, tone: 60, lyric: '+*', phoneme_overrides: [{{index: 0, phoneme: a}}]}}\n",
            i * 480
        ));
    }
    let imported = ustx::import(yaml.as_bytes()).expect("imports");
    let count = |code: &str| codes(&imported).iter().filter(|c| **c == code).count();
    assert_eq!(count("ustx.lyric_extension"), 1);
    assert_eq!(count("ustx.phoneme_override_unsupported"), 1);
    let ext = imported
        .diagnostics
        .iter()
        .find(|d| d.code == "ustx.lyric_extension")
        .unwrap();
    assert!(ext.message.contains("500 notes"), "{}", ext.message);
}

#[test]
fn parts_on_missing_tracks_are_reported_not_fatal() {
    let yaml =
        b"ustx_version: '0.6'\ntracks: []\nvoice_parts:\n- {track_no: 3, position: 0, notes: []}\n";
    let imported = ustx::import(yaml).expect("imports");
    assert_eq!(
        codes(&imported),
        [
            "ustx.tempo_missing",
            "ustx.time_sig_missing_start",
            "ustx.part_track_missing"
        ]
    );
    assert!(imported.project.tracks.is_empty());
}
