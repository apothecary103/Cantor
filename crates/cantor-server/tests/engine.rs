//! Engine API tests with the real voicebank.

mod common;

use cantor_core::*;
use cantor_render::render::Quality;
use cantor_server::EngineError;
use cantor_server::describe::Detail;
use std::collections::BTreeMap;

#[test]
fn describe_and_conflicts() {
    let e = common::engine("describe");
    let info = e.open_project(&common::twinkle_file("describe")).unwrap();
    let text = e.describe_project(&info.project_id).unwrap();
    eprintln!("{text}");
    assert!(
        text.contains("\"yi\" /zh/y zh/i/") || text.contains("\"yi\" /"),
        "{text}"
    );
    assert!(text.contains("Phrase 104"));

    let r = e
        .apply_ops(
            &info.project_id,
            Revision(0),
            vec![Op::SetLyric {
                id: NoteId(100),
                lyric: "la".into(),
            }],
            false,
        )
        .unwrap();
    assert!(r.applied);
    // A stale base revision is a conflict carrying what changed.
    let err = e
        .apply_ops(
            &info.project_id,
            Revision(0),
            vec![Op::SetLyric {
                id: NoteId(101),
                lyric: "la".into(),
            }],
            false,
        )
        .unwrap_err();
    match err {
        EngineError::Conflict { current, diff, .. } => {
            assert_eq!(current, Revision(1));
            assert!(diff.modified.contains(&EntityRef::Note { id: NoteId(100) }));
        }
        other => panic!("{other}"),
    }
    let range = e
        .describe_range(
            &info.project_id,
            None,
            TickRange {
                start: 0,
                end: 1920,
            },
            Detail::Notes,
        )
        .unwrap();
    assert!(range.contains("#100") && !range.contains("#101"), "{range}");

    e.checkpoint(&info.project_id, "after-la").unwrap();
    let singers = e.list_singers();
    assert_eq!(singers.len(), 1);
    let schema = e.get_singer_schema(&singers[0].id).unwrap();
    assert!(
        schema
            .parameters
            .iter()
            .any(|p| p.descriptor.id == "breathiness" && p.supported)
    );
    assert!(
        schema
            .parameters
            .iter()
            .any(|p| p.descriptor.id == "tension" && !p.supported)
    );
}

/// The closed loop the MCP agent test repeats: measure, correct the pitch
/// curve where the voice is off, re-render (vocoder fast path).
#[test]
fn tuning_loop_lowers_p95_below_20_cents() {
    let e = common::engine("tune");
    let info = e.open_project(&common::twinkle_file("tune")).unwrap();
    let id = info.project_id;
    let phrase = TickRange {
        start: 0,
        end: 4 * 1920,
    };
    let mut applied: BTreeMap<i64, f64> = BTreeMap::new();
    let mut history = Vec::new();
    for round in 0..6 {
        let r = e.render(&id, Some(phrase), Quality::Preview).unwrap();
        let a = r.analysis.as_ref().unwrap();
        let p95 = a.tracks[0].pitch.p95_cents.unwrap();
        eprintln!(
            "round {round}: p95 {p95:.1} cents, trace {:?}",
            r.trace
                .phrases
                .iter()
                .map(|t| t.outcome)
                .collect::<Vec<_>>()
        );
        history.push(p95);
        if p95 < 20.0 {
            break;
        }
        let (_, rev) = e.snapshot(&id).unwrap();
        let mut ops = Vec::new();
        for n in a.notes().filter(|n| n.error_contour.len() >= 2) {
            let c = &n.error_contour;
            for p in c {
                *applied.entry(p.tick).or_insert(0.0) -= p.cents;
            }
            let (first, last) = (c[0].tick, c[c.len() - 1].tick);
            let mut points = vec![(first - 80, 0.0)];
            points.extend(applied.range(first..=last).map(|(t, v)| (*t, *v)));
            points.push((last + 80, 0.0));
            ops.push(Op::SetCurve {
                track: TrackId(1),
                param: ParamId::new("pitch"),
                range: TickRange {
                    start: first - 80,
                    end: last + 81,
                },
                points,
                interp: Interp::Linear,
            });
        }
        let res = e.apply_ops(&id, rev, ops, false).unwrap();
        assert!(res.applied, "{:?}", res.diagnostics);
    }
    assert!(*history.last().unwrap() < 20.0, "p95 history {history:?}");
    assert!(history[0] > history[history.len() - 1]);
}
