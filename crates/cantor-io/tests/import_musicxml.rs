//! MusicXML importer tests.

use cantor_io::import::{Format, ImportError, Imported, import};
use cantor_io::project_file::{Compression, from_bytes, to_bytes};
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
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/musicxml")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn import_xml(bytes: &[u8]) -> Result<Imported, ImportError> {
    import(bytes, Format::MusicXml)
}

fn snapshot(name: &str) -> Imported {
    let imported = import_xml(&fixture(&format!("{name}.musicxml"))).expect("imports");
    assert_snapshot_sorted!(
        name,
        serde_json::json!({
            "project": imported.project,
            "diagnostics": imported.diagnostics,
        })
    );
    imported
}

fn codes(imported: &Imported) -> Vec<&str> {
    imported
        .diagnostics
        .iter()
        .map(|d| d.code.as_str())
        .collect()
}

/// Wraps measures of one part in a minimal score.
fn score(measures: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<score-partwise version="4.0">
  <part-list><score-part id="P1"><part-name>V</part-name></score-part></part-list>
  <part id="P1">{measures}</part>
</score-partwise>"#
    )
}

#[test]
fn duet() {
    snapshot("duet");
}

#[test]
fn minimal() {
    snapshot("minimal");
}

#[test]
fn round_trips_through_project_file() {
    for name in ["duet", "minimal"] {
        let imported = import_xml(&fixture(&format!("{name}.musicxml"))).expect("imports");
        let bytes = to_bytes(&imported.project, Compression::None);
        let loaded = from_bytes(&bytes).expect("loads");
        assert_eq!(loaded.project, imported.project, "{name}");
        assert!(
            loaded.diagnostics.is_empty(),
            "{name}: {:?}",
            loaded.diagnostics
        );
        assert_eq!(loaded.migrated_from, None);
    }
}

#[test]
fn import_is_deterministic() {
    let a = import_xml(&fixture("duet.musicxml")).expect("imports");
    let b = import_xml(&fixture("duet.musicxml")).expect("imports");
    assert_eq!(a, b);
}

#[test]
fn malformed_xml_is_a_parse_error() {
    let cases: [&[u8]; 5] = [
        b"<score-partwise><part-list></score-partwise>",
        b"<score-partwise><part-list>",
        b"not xml at all",
        b"",
        b"\xff\xfe\x00garbage",
    ];
    for bytes in cases {
        let result = import_xml(bytes);
        assert!(
            matches!(
                result,
                Err(ImportError::Parse { .. } | ImportError::Unsupported { .. })
            ),
            "{:?}: {result:?}",
            String::from_utf8_lossy(bytes)
        );
    }
    let result = import_xml(b"<score-partwise><part-list></score-partwise>");
    assert!(
        matches!(result, Err(ImportError::Parse { .. })),
        "{result:?}"
    );
}

#[test]
fn bad_numbers_are_parse_errors() {
    let xml =
        score("<measure number=\"1\"><attributes><divisions>x</divisions></attributes></measure>");
    assert!(matches!(
        import_xml(xml.as_bytes()),
        Err(ImportError::Parse { .. })
    ));
    let xml = score(
        "<measure number=\"1\"><attributes><divisions>1</divisions></attributes>\
         <note><pitch><step>C</step><octave>4</octave></pitch></note></measure>",
    );
    assert!(matches!(
        import_xml(xml.as_bytes()),
        Err(ImportError::Parse { .. })
    ));
}

#[test]
fn timewise_is_unsupported() {
    let xml = br#"<?xml version="1.0"?>
<score-timewise version="4.0"><part-list/></score-timewise>"#;
    assert!(matches!(
        import_xml(xml),
        Err(ImportError::Unsupported { .. })
    ));
}

#[test]
fn compressed_mxl_is_unsupported() {
    let result = import_xml(b"PK\x03\x04rest of a zip archive");
    match result {
        Err(ImportError::Unsupported { message, .. }) => {
            assert!(
                message.contains("unzip") || message.contains("extract"),
                "{message}"
            );
        }
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

#[test]
fn doctype_with_entities_does_not_fail() {
    let xml = r#"<?xml version="1.0"?>
<!DOCTYPE score-partwise SYSTEM "http://example.invalid/partwise.dtd" [<!ENTITY x "y">]>
<score-partwise version="4.0">
  <work><work-title>A &amp; B &x; &#233;</work-title></work>
  <part-list><score-part id="P1"><part-name>V</part-name></score-part></part-list>
  <part id="P1"><measure number="1"/></part>
</score-partwise>"#;
    let imported = import_xml(xml.as_bytes()).expect("imports");
    assert_eq!(imported.project.meta.title, "A & B &x; é");
}

#[test]
fn inexact_divisions_are_rounded_once() {
    let xml = score(
        "<measure number=\"1\"><attributes><divisions>7</divisions>\
         <time><beats>4</beats><beat-type>4</beat-type></time></attributes>\
         <sound tempo=\"100\"/>\
         <note><pitch><step>C</step><octave>4</octave></pitch><duration>1</duration></note>\
         <note><pitch><step>D</step><octave>4</octave></pitch><duration>1</duration></note>\
         <note><rest/><duration>26</duration></note></measure>",
    );
    let imported = import_xml(xml.as_bytes()).expect("imports");
    let notes = &imported.project.tracks[0].parts[0].notes;
    assert_eq!((notes[0].start, notes[0].length), (0, 274));
    assert_eq!((notes[1].start, notes[1].length), (274, 275));
    assert_eq!(
        codes(&imported)
            .iter()
            .filter(|c| **c == "musicxml.rounding")
            .count(),
        1
    );
}

#[test]
fn unpitched_and_cue_notes_are_skipped_with_diagnostics() {
    let xml = score(
        "<measure number=\"1\"><attributes><divisions>1</divisions>\
         <time><beats>3</beats><beat-type>4</beat-type></time></attributes>\
         <sound tempo=\"100\"/>\
         <note><pitch><step>C</step><octave>4</octave></pitch><duration>1</duration>\
         <lyric><text>do</text></lyric></note>\
         <note><unpitched><display-step>E</display-step><display-octave>4</display-octave></unpitched><duration>1</duration></note>\
         <note><cue/><pitch><step>E</step><octave>4</octave></pitch><duration>1</duration></note>\
         </measure>",
    );
    let imported = import_xml(xml.as_bytes()).expect("imports");
    assert_eq!(imported.project.tracks[0].parts[0].notes.len(), 1);
    let codes = codes(&imported);
    assert!(codes.contains(&"musicxml.unpitched"), "{codes:?}");
    assert!(codes.contains(&"musicxml.cue"), "{codes:?}");
}

#[test]
fn transposed_part_sounds_at_concert_pitch() {
    let xml = score(
        "<measure number=\"1\"><attributes><divisions>1</divisions>\
         <key><fifths>2</fifths></key>\
         <time><beats>1</beats><beat-type>4</beat-type></time>\
         <transpose><diatonic>-1</diatonic><chromatic>-2</chromatic></transpose></attributes>\
         <sound tempo=\"100\"/>\
         <note><pitch><step>D</step><octave>4</octave></pitch><duration>1</duration>\
         <lyric><text>do</text></lyric></note></measure>",
    );
    let imported = import_xml(xml.as_bytes()).expect("imports");
    assert_eq!(
        imported.project.tracks[0].parts[0].notes[0].pitch_cents,
        6000.0
    );
    assert_eq!(imported.project.key_map[0].tonic, 0); // D major written = C major sounding
    assert!(codes(&imported).contains(&"musicxml.transpose"));
}
