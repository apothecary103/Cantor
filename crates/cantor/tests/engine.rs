//! The engine as the HTTP API and MCP use it, with the generated CC0 bank.

use cantor::engine::{Caller, Engine, Source};
use cantor_core::Op;
use serde_json::json;
use std::path::PathBuf;
use std::sync::OnceLock;

fn banks_dir() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("cantor-banks");
        cantor_fixtures::utau::ensure_synthetic_voicebank(&root.join("cantor-synthetic"))
            .expect("fixture bank");
        root
    })
    .clone()
}

fn op(v: serde_json::Value) -> Op {
    serde_json::from_value(v).expect("valid op")
}

fn engine_with_song() -> Engine {
    let e = Engine::new(vec![banks_dir()]);
    let t = e.project().tracks[0].id;
    let notes: Vec<_> = ["か", "き", "く", "け", "こ", "あ"]
        .iter()
        .enumerate()
        .map(|(i, l)| json!({ "start": i * 480, "length": 480, "key": 60 + i, "lyric": l }))
        .collect();
    e.apply_ops(
        vec![op(json!({ "op": "add_notes", "track": t, "notes": notes }))],
        None,
        &Caller::default(),
    )
    .expect("adds");
    e
}

#[test]
fn new_engine_has_a_track_on_the_first_voicebank() {
    let e = Engine::new(vec![banks_dir()]);
    let p = e.project();
    assert_eq!(p.tracks.len(), 1);
    assert_ne!(p.tracks[0].id.0, 0);
    assert_eq!(p.tracks[0].singer, "cantor-synthetic");
    assert_eq!(e.voicebanks().len(), 1);
}

#[test]
fn history_records_sources_and_checkpoints_restore() {
    let e = engine_with_song();
    let t = e.project().tracks[0].id;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    e.subscribe(tx);
    e.checkpoint("draft", &Caller::mcp()).expect("checkpoint");
    let req = serde_json::from_value(
        json!({ "pass": "auto_pitch", "track": t, "params": { "style": "pop" } }),
    )
    .expect("request");
    let r = e.tune(req, &Caller::mcp()).expect("tunes");
    assert!(r.ops_applied > 0);
    let h = e.history();
    assert_eq!(
        h.undo.iter().map(|s| s.source).collect::<Vec<_>>(),
        [Source::Http, Source::Mcp]
    );
    assert_eq!(h.checkpoints, ["draft"]);

    // Events name the source.
    let events: Vec<serde_json::Value> = std::iter::from_fn(|| rx.try_recv().ok())
        .map(|s| serde_json::from_str(&s).expect("json"))
        .collect();
    assert_eq!(events[0]["type"], "checkpoint");
    assert_eq!(events[1]["type"], "revision");
    assert_eq!(events[1]["source"], "mcp");

    // Restore is one undoable step; undo moves sources along.
    let tuned = e.project();
    e.restore("draft", &Caller::default()).expect("restores");
    assert_ne!(e.project(), tuned);
    e.undo_redo(false, 1, &Caller::default()).expect("undo");
    assert_eq!(e.project(), tuned);
    let h = e.history();
    assert_eq!(h.redo.len(), 1);
    assert_eq!(h.redo[0].source, Source::Http);
    e.undo_redo(false, 5, &Caller::default()).expect("undo all");
    assert!(e.project().tracks[0].notes.is_empty());
    assert_eq!(e.history().redo.len(), 3);
}

#[test]
fn stale_revisions_are_rejected() {
    let e = engine_with_song();
    let t = e.project().tracks[0].id;
    let stale = cantor_core::Revision(0);
    let err = e
        .apply_ops(
            vec![op(
                json!({ "op": "update_track", "track": t, "patch": { "name": "x" } }),
            )],
            Some(stale),
            &Caller::default(),
        )
        .unwrap_err();
    assert_eq!(err.code, "revision_conflict");
}

#[test]
fn renders_with_report_and_cache() {
    let e = engine_with_song();
    let r = e.render(None, &Caller::default()).expect("renders");
    assert!(r.report.missing_aliases.is_empty());
    assert!(r.report.pitch_error.is_some());
    assert!(!e.render_wav(&r.render_id).expect("kept").is_empty());
    let again = e.render(None, &Caller::default()).expect("renders");
    assert_eq!(again.phrases_rendered, 0);
    let ph = e.phonemes(e.project().tracks[0].id).expect("phonemes");
    assert_eq!(ph.len(), 6);
    assert!(ph.iter().all(|p| !p.missing));
}

#[test]
fn every_web_file_is_embedded() {
    let web = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../web");
    let mut files = Vec::new();
    for dir in [web.clone(), web.join("js")] {
        for f in std::fs::read_dir(&dir).expect("web dir").flatten() {
            if f.path().is_file() {
                files.push(
                    f.path()
                        .strip_prefix(&web)
                        .expect("inside")
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    for f in &files {
        assert!(
            cantor::http::WEB.iter().any(|(p, _, _)| p == f),
            "web/{f} is not embedded in http::WEB"
        );
    }
    assert_eq!(files.len(), cantor::http::WEB.len());
}
