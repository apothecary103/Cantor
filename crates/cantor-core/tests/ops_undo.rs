//! Property tests for the Op system and the undo tree.
//!
//! Random op sequences (valid and invalid) are generated as abstract
//! "seeds" and resolved against the current document, so most ops address
//! real ids while some deliberately miss. The main property: applying any
//! sequence and then undoing everything restores a byte-identical project.

use cantor_core::*;
use proptest::prelude::*;
use std::collections::BTreeMap;

/// An op before it is bound to the ids of a concrete document.
#[derive(Clone, Debug)]
enum Seed {
    SetMeta(String),
    SetTempo(i64, f64),
    RemoveTempo(usize),
    SetTimeSig(u32, u8, u8),
    RemoveTimeSig(usize),
    SetKey(i64, u8, usize),
    RemoveKey(usize),
    PhraseNoise(u64, Option<u64>),
    SelectTake(u64, Option<u64>),
    AddTrack(Option<usize>, bool),
    RemoveTrack(usize),
    MoveTrack(usize, usize),
    SetTrack(usize, f64, f64, bool, u8),
    AddPart(usize, i64, i64, bool),
    RemovePart(usize),
    MovePart(usize, i64, bool),
    SetPart(usize, Option<String>, Option<i64>),
    AddNote(usize, i64, i64, f64, String),
    RemoveNotes(Vec<usize>),
    MoveNotes(Vec<usize>, i64, f64, bool),
    ResizeNote(usize, i64),
    SetLyric(usize, String),
    SetPhonemes(usize, Option<Vec<String>>),
    SetNoteExpr(usize, Option<Option<(f64, f64)>>, Option<f64>),
    SetVocalModes(usize, f64),
    SetCurve(usize, usize, i64, i64, Vec<(i64, f64)>, usize),
    ClearCurve(usize, usize),
    Batch(Vec<Seed>),
}

const PARAMS: &[&str] = &["pitch", "breathiness", "tension", "gender", "not_a_param"];
const SCALES: &[Scale] = &[Scale::Major, Scale::Minor, Scale::Dorian, Scale::Chromatic];
const INTERPS: &[Interp] = &[Interp::Step, Interp::Linear, Interp::MonotoneHermite];

fn lyric() -> impl Strategy<Value = String> {
    prop::sample::select(vec!["la", "a", "+", "hel-", "-lo", "ka", "ん", ""]).prop_map(String::from)
}

/// Pitches, including low ones: with |delta| > |pitch|, `(p + d) - d` is
/// usually not `p` in floating point, so undo must restore stored values
/// rather than negate deltas.
fn pitch() -> impl Strategy<Value = f64> {
    prop_oneof![-100.0f64..13_000.0, 0.0f64..64.0]
}

fn pitch_delta() -> impl Strategy<Value = f64> {
    prop_oneof![-1234.567f64..1234.567, 100.0f64..9_000.0, -1e-3f64..1e-3]
}

fn leaf() -> impl Strategy<Value = Seed> {
    let idx = || 0usize..64;
    let tick = || -2_000i64..80_000;
    let len = || -10i64..8_000;
    prop_oneof![
        "[a-z ]{0,8}".prop_map(Seed::SetMeta),
        (tick(), 0.5f64..1100.0).prop_map(|(t, b)| Seed::SetTempo(t, b)),
        idx().prop_map(Seed::RemoveTempo),
        (
            0u32..12,
            0u8..9,
            prop::sample::select(vec![1u8, 2, 3, 4, 8, 16])
        )
            .prop_map(|(b, n, d)| Seed::SetTimeSig(b, n, d)),
        idx().prop_map(Seed::RemoveTimeSig),
        (tick(), 0u8..13, idx()).prop_map(|(t, k, s)| Seed::SetKey(t, k, s)),
        idx().prop_map(Seed::RemoveKey),
        (0u64..4, prop::option::of(any::<u64>())).prop_map(|(p, s)| Seed::PhraseNoise(p, s)),
        (0u64..4, prop::option::of(0u64..3)).prop_map(|(p, t)| Seed::SelectTake(p, t)),
        (prop::option::of(0usize..4), any::<bool>()).prop_map(|(i, c)| Seed::AddTrack(i, c)),
        idx().prop_map(Seed::RemoveTrack),
        (idx(), 0usize..3).prop_map(|(a, b)| Seed::MoveTrack(a, b)),
        (idx(), -60.0f64..12.0, -1.2f64..1.2, any::<bool>(), 0u8..18)
            .prop_map(|(t, g, p, m, l)| Seed::SetTrack(t, g, p, m, l)),
        (idx(), tick(), len(), any::<bool>()).prop_map(|(t, s, l, n)| Seed::AddPart(t, s, l, n)),
        idx().prop_map(Seed::RemovePart),
        (idx(), -20_000i64..20_000, any::<bool>()).prop_map(|(p, d, c)| Seed::MovePart(p, d, c)),
        (
            idx(),
            prop::option::of("[A-Z][a-z]{0,5}"),
            prop::option::of(len())
        )
            .prop_map(|(p, n, l)| Seed::SetPart(p, n, l)),
        (idx(), -100i64..20_000, len(), pitch(), lyric())
            .prop_map(|(p, s, l, c, y)| Seed::AddNote(p, s, l, c, y)),
        prop::collection::vec(idx(), 0..4).prop_map(Seed::RemoveNotes),
        (
            prop::collection::vec(idx(), 1..4),
            -1_000i64..4_000,
            pitch_delta(),
            any::<bool>()
        )
            .prop_map(|(n, d, c, k)| Seed::MoveNotes(n, d, c, k)),
        (idx(), len()).prop_map(|(n, l)| Seed::ResizeNote(n, l)),
        (idx(), lyric()).prop_map(|(n, l)| Seed::SetLyric(n, l)),
        (
            idx(),
            prop::option::of(prop::collection::vec("[a-z]{0,2}", 0..3))
        )
            .prop_map(|(n, p)| Seed::SetPhonemes(n, p)),
        (
            idx(),
            prop::option::of(prop::option::of((0.0f64..120.0, 3.0f64..8.0))),
            prop::option::of(0.0f64..1.0)
        )
            .prop_map(|(n, v, b)| Seed::SetNoteExpr(n, v, b)),
        (idx(), -2.0f64..2.0).prop_map(|(n, w)| Seed::SetVocalModes(n, w)),
        (
            idx(),
            0usize..PARAMS.len(),
            -100i64..40_000,
            -10i64..6_000,
            prop::collection::vec((0i64..6_000, -1.1f64..1.1), 0..6),
            0usize..INTERPS.len()
        )
            .prop_map(|(t, p, s, l, pts, i)| Seed::SetCurve(t, p, s, l, pts, i)),
        (idx(), 0usize..PARAMS.len()).prop_map(|(t, p)| Seed::ClearCurve(t, p)),
    ]
}

/// Mostly single ops; some batches, including nested ones, which also
/// exercise rollback when a later op in a batch fails.
fn seed() -> impl Strategy<Value = Seed> {
    let batch = || prop::collection::vec(leaf(), 0..4).prop_map(Seed::Batch);
    prop_oneof![
        8 => leaf(),
        1 => batch(),
        1 => prop::collection::vec(prop_oneof![3 => leaf(), 1 => batch()], 0..3)
            .prop_map(Seed::Batch),
    ]
}

/// Picks an existing id, or (one time in `len + 1`) an id that does not
/// exist, so invalid ops are exercised too.
fn pick<T: Copy>(ids: &[T], i: usize, missing: T) -> T {
    let k = i % (ids.len() + 1);
    if k == ids.len() { missing } else { ids[k] }
}

fn note(id: NoteId, start: Tick, length: Tick, pitch_cents: f64, lyric: String) -> Note {
    Note {
        id,
        start,
        length,
        pitch_cents,
        lyric,
        lang: None,
        phonemes: None,
        expr: NoteExpr::default(),
        modes: VocalModes::default(),
    }
}

/// Binds a seed to the document's current ids. `next` hands out fresh ids
/// for new entities; it only grows, so ids never collide across branches.
fn resolve(seed: &Seed, p: &Project, next: &mut u64) -> Op {
    let tracks: Vec<TrackId> = p.tracks.iter().map(|t| t.id).collect();
    let parts: Vec<PartId> = p
        .tracks
        .iter()
        .flat_map(|t| &t.parts)
        .map(|x| x.id)
        .collect();
    let notes: Vec<NoteId> = p.notes().map(|(_, _, n)| n.id).collect();
    let mut fresh = || {
        *next += 1;
        *next
    };
    let track = |i| pick(&tracks, i, TrackId(u64::MAX));
    let part = |i| pick(&parts, i, PartId(u64::MAX));
    let note_id = |i| pick(&notes, i, NoteId(u64::MAX));
    match seed {
        Seed::SetMeta(t) => Op::SetMeta {
            meta: Meta {
                title: t.clone(),
                ..p.meta.clone()
            },
        },
        Seed::SetTempo(tick, bpm) => Op::SetTempo {
            tick: *tick,
            bpm: *bpm,
        },
        Seed::RemoveTempo(i) => Op::RemoveTempo {
            tick: pick(
                &p.tempo_map.iter().map(|t| t.tick).collect::<Vec<_>>(),
                *i,
                7,
            ),
        },
        Seed::SetTimeSig(bar, n, d) => Op::SetTimeSig {
            bar: *bar,
            numerator: *n,
            denominator: *d,
        },
        Seed::RemoveTimeSig(i) => Op::RemoveTimeSig {
            bar: pick(
                &p.time_sig_map.iter().map(|t| t.bar).collect::<Vec<_>>(),
                *i,
                99,
            ),
        },
        Seed::SetKey(tick, tonic, s) => Op::SetKey {
            tick: *tick,
            tonic: *tonic,
            scale: SCALES[s % SCALES.len()],
        },
        Seed::RemoveKey(i) => Op::RemoveKey {
            tick: pick(&p.key_map.iter().map(|k| k.tick).collect::<Vec<_>>(), *i, 3),
        },
        Seed::PhraseNoise(phrase, s) => Op::SetSeed {
            phrase: PhraseId(*phrase),
            seed: *s,
        },
        Seed::SelectTake(phrase, t) => Op::SelectTake {
            phrase: PhraseId(*phrase),
            take: t.map(TakeId),
        },
        Seed::AddTrack(index, with_content) => {
            let id = TrackId(fresh());
            let mut t = Track {
                id,
                name: format!("Track {}", id.0),
                singer: SingerRef {
                    id: "test-singer".into(),
                    version: None,
                },
                backend: BackendRef("diffsinger".into()),
                mix: Mix::default(),
                effects: vec![],
                doubling: Doubling::default(),
                voice_mix: vec![],
                curves: BTreeMap::new(),
                parts: vec![],
            };
            if *with_content {
                let n1 = note(NoteId(fresh()), 0, 960, 6000.25, "a".into());
                let n2 = note(NoteId(fresh()), 960, 960, 6200.0, "+".into());
                t.parts.push(Part {
                    id: PartId(fresh()),
                    name: "Intro".into(),
                    start: 1920,
                    length: 7680,
                    notes: vec![n1, n2],
                });
                t.curves.insert(
                    ParamId::new("pitch"),
                    LayeredCurve {
                        model_prediction: Curve {
                            points: vec![(1920, 1.5), (2400, -3.25)],
                            interp: Interp::MonotoneHermite,
                        },
                        user_delta: Curve {
                            points: vec![(2000, 10.0)],
                            interp: Interp::Linear,
                        },
                    },
                );
            }
            Op::AddTrack {
                index: *index,
                track: t,
            }
        }
        Seed::RemoveTrack(i) => Op::RemoveTrack { id: track(*i) },
        Seed::MoveTrack(i, index) => Op::MoveTrack {
            id: track(*i),
            index: *index,
        },
        Seed::SetTrack(i, gain, pan, mute, layers) => Op::SetTrack {
            id: track(*i),
            patch: TrackPatch {
                mix: Some(Mix {
                    gain_db: *gain,
                    pan: *pan,
                    mute: *mute,
                    solo: false,
                }),
                doubling: (*layers != 0).then(|| Doubling {
                    layers: *layers,
                    ..Doubling::default()
                }),
                ..TrackPatch::default()
            },
        },
        Seed::AddPart(i, start, length, with_note) => {
            let id = PartId(fresh());
            let notes = if *with_note {
                vec![note(NoteId(fresh()), 0, 480, 5900.0, "la".into())]
            } else {
                vec![]
            };
            Op::AddPart {
                track: track(*i),
                part: Part {
                    id,
                    name: "P".into(),
                    start: *start,
                    length: *length,
                    notes,
                },
            }
        }
        Seed::RemovePart(i) => Op::RemovePart { id: part(*i) },
        Seed::MovePart(i, dt, carry) => Op::MovePart {
            id: part(*i),
            dt: *dt,
            carry_curves: *carry,
        },
        Seed::SetPart(i, name, length) => Op::SetPart {
            id: part(*i),
            name: name.clone(),
            length: *length,
        },
        Seed::AddNote(i, start, length, pitch, lyric) => Op::AddNote {
            part: part(*i),
            note: note(NoteId(fresh()), *start, *length, *pitch, lyric.clone()),
        },
        Seed::RemoveNotes(is) => Op::RemoveNotes {
            ids: is.iter().map(|i| note_id(*i)).collect(),
        },
        Seed::MoveNotes(is, dt, dcents, carry) => Op::MoveNotes {
            ids: is.iter().map(|i| note_id(*i)).collect(),
            dt: *dt,
            dcents: *dcents,
            carry_curves: *carry,
        },
        Seed::ResizeNote(i, length) => Op::ResizeNote {
            id: note_id(*i),
            length: *length,
        },
        Seed::SetLyric(i, lyric) => Op::SetLyric {
            id: note_id(*i),
            lyric: lyric.clone(),
        },
        Seed::SetPhonemes(i, ph) => Op::SetPhonemes {
            id: note_id(*i),
            phonemes: ph.as_ref().map(|v| {
                v.iter()
                    .enumerate()
                    .map(|(k, s)| PhonemeOverride {
                        symbol: s.clone(),
                        alphabet: if k % 2 == 0 {
                            PhonemeAlphabet::Native
                        } else {
                            PhonemeAlphabet::XSampa
                        },
                        offset_ms: (k == 0).then_some(-35.5),
                        length_ms: (k == 1).then_some(60.125),
                    })
                    .collect()
            }),
        },
        Seed::SetNoteExpr(i, vib, before) => Op::SetNoteExpr {
            id: note_id(*i),
            expr: NoteExprPatch {
                vibrato: vib.map(|v| {
                    v.map(|(depth_cents, rate_hz)| Vibrato {
                        depth_cents,
                        rate_hz,
                        start: 0.3,
                        fade_in: 0.2,
                        fade_out: 0.1,
                        phase: 0.0,
                        drift: 0.5,
                    })
                }),
                portamento: before.map(|b| Portamento {
                    before: b,
                    ..Portamento::default()
                }),
                attack: None,
                release: Some(Some(Envelope {
                    length_ms: 80.0,
                    strength: 0.5,
                })),
            },
        },
        Seed::SetVocalModes(i, w) => Op::SetVocalModes {
            id: note_id(*i),
            modes: VocalModes {
                pitch: BTreeMap::from([("soft".into(), *w)]),
                ..VocalModes::default()
            },
        },
        Seed::SetCurve(i, param, start, len, pts, interp) => {
            // Values are fractions of the parameter's delta limit; a few
            // exceed it on purpose.
            let limit = ParamId::new(PARAMS[*param])
                .descriptor()
                .map_or(100.0, |d| d.max_delta());
            let mut points: Vec<(Tick, f64)> = pts
                .iter()
                .map(|&(o, v)| (start + o % (*len).max(1), v * limit))
                .collect();
            points.sort_by_key(|p| p.0);
            points.dedup_by_key(|p| p.0);
            Op::SetCurve {
                track: track(*i),
                param: ParamId::new(PARAMS[*param]),
                range: TickRange {
                    start: *start,
                    end: start + len,
                },
                points,
                interp: INTERPS[*interp],
            }
        }
        Seed::ClearCurve(i, param) => Op::RestoreCurve {
            track: track(*i),
            param: ParamId::new(PARAMS[*param]),
            curve: None,
        },
        Seed::Batch(seeds) => {
            // Resolve against a scratch copy so later ops in the batch can
            // address entities created by earlier ones.
            let mut scratch = Document::new(p.clone());
            let ops: Vec<Op> = seeds
                .iter()
                .map(|s| {
                    let op = resolve(s, scratch.project(), next);
                    scratch.apply(op.clone());
                    op
                })
                .collect();
            Op::Batch {
                label: "batch".into(),
                ops,
            }
        }
    }
}

fn bytes(p: &Project) -> Vec<u8> {
    serde_json::to_vec(p).expect("projects serialize")
}

/// A starting project with some content, built through ops.
fn initial_project(seeds: &[Seed]) -> (Project, u64) {
    let mut next = 1000;
    let mut doc = Document::new(Project::new());
    let base = [
        Seed::AddTrack(None, true),
        Seed::SetTimeSig(4, 3, 4),
        Seed::SetTempo(7680, 92.5),
        Seed::SetKey(0, 9, 1),
    ];
    for s in base.iter().chain(seeds) {
        let op = resolve(s, doc.project(), &mut next);
        doc.apply(op);
    }
    ((*doc.snapshot()).clone(), next)
}

/// Applies seeds one by one, checking per-op properties. Returns how many
/// ops were applied.
fn apply_all(doc: &mut Document, seeds: &[Seed], next: &mut u64) -> Result<usize, TestCaseError> {
    let mut applied = 0;
    for s in seeds {
        let op = resolve(s, doc.project(), next);

        let json = serde_json::to_string(&op).expect("ops serialize");
        let back: Op = serde_json::from_str(&json).expect("ops deserialize");
        prop_assert_eq!(&back, &op, "op JSON round trip");

        let before = doc.revision();
        let dry = doc.dry_run(&op);
        let r = doc.apply(op);
        prop_assert_eq!(dry.applied, r.applied, "dry run agrees with apply");
        prop_assert_eq!(&dry.diff, &r.diff, "dry run diff");
        if r.applied {
            applied += 1;
            prop_assert_eq!(r.revision.0, before.0 + 1);
            prop_assert!(!r.diagnostics.iter().any(Diagnostic::is_error));
        } else {
            prop_assert_eq!(r.revision, before);
            prop_assert!(
                r.diagnostics.iter().any(Diagnostic::is_error),
                "rejections say why"
            );
            prop_assert!(r.diff.is_empty());
        }
        let errors: Vec<_> = doc
            .project()
            .validate()
            .into_iter()
            .filter(Diagnostic::is_error)
            .collect();
        prop_assert!(errors.is_empty(), "invariants hold: {:?}", errors);
    }
    Ok(applied)
}

fn cases() -> u32 {
    if cfg!(miri) { 4 } else { 512 }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: cases(),
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// Apply any op sequence, undo everything: the project is
    /// byte-identical to the start. Redo everything: byte-identical to the
    /// end state.
    #[test]
    fn apply_then_undo_restores_identical_bytes(
        setup in prop::collection::vec(leaf(), 0..12),
        seeds in prop::collection::vec(seed(), 1..40),
    ) {
        let (start, mut next) = initial_project(&setup);
        let start_bytes = bytes(&start);
        let mut doc = Document::new(start);

        let applied = apply_all(&mut doc, &seeds, &mut next)?;
        let end_bytes = bytes(doc.project());

        let mut undone = 0;
        while doc.undo().is_some() {
            undone += 1;
        }
        prop_assert_eq!(undone, applied);
        prop_assert!(bytes(doc.project()) == start_bytes, "undo restores the start");

        let mut redone = 0;
        while doc.redo().is_some() {
            redone += 1;
        }
        prop_assert_eq!(redone, applied);
        prop_assert!(bytes(doc.project()) == end_bytes, "redo restores the end");
    }

    /// Branches survive: undo part way, make different edits, then check
    /// out the old branch, the new branch and the root.
    #[test]
    fn branches_can_be_checked_out(
        first in prop::collection::vec(seed(), 1..20),
        back in 0usize..20,
        second in prop::collection::vec(seed(), 1..20),
    ) {
        let (start, mut next) = initial_project(&[]);
        let start_bytes = bytes(&start);
        let mut doc = Document::new(start);

        apply_all(&mut doc, &first, &mut next)?;
        let a_node = doc.history().current();
        let a_bytes = bytes(doc.project());
        for _ in 0..back {
            doc.undo();
        }
        apply_all(&mut doc, &second, &mut next)?;
        let b_node = doc.history().current();
        let b_bytes = bytes(doc.project());

        doc.checkout(a_node).expect("node exists");
        prop_assert!(bytes(doc.project()) == a_bytes, "branch A restored");
        doc.checkout(b_node).expect("node exists");
        prop_assert!(bytes(doc.project()) == b_bytes, "branch B restored");
        doc.checkout(HistoryNodeId::ROOT).expect("root exists");
        prop_assert!(bytes(doc.project()) == start_bytes, "root restored");
        prop_assert_eq!(doc.history().redo_target().is_some(), doc.history().nodes().len() > 1);
    }
}

#[test]
fn rejected_op_reports_and_changes_nothing() {
    let mut doc = Document::new(Project::new());
    let before = bytes(doc.project());
    let r = doc.apply(Op::SetLyric {
        id: NoteId(42),
        lyric: "la".into(),
    });
    assert!(!r.applied);
    assert_eq!(r.diagnostics[0].code, "note.not_found");
    assert_eq!(bytes(doc.project()), before);
    assert!(!doc.history().can_undo());
}

#[test]
fn unknown_parameter_is_a_diagnostic() {
    let mut next = 0;
    let mut doc = Document::new(Project::new());
    doc.apply(resolve(
        &Seed::AddTrack(None, false),
        doc.project(),
        &mut next,
    ));
    let id = doc.project().tracks[0].id;
    let r = doc.apply(Op::SetCurve {
        track: id,
        param: ParamId::new("growl"),
        range: TickRange { start: 0, end: 10 },
        points: vec![(0, 1.0)],
        interp: Interp::Linear,
    });
    assert!(!r.applied);
    assert_eq!(r.diagnostics[0].code, "curve.unknown_param");
}

#[test]
fn batch_is_atomic() {
    let mut next = 0;
    let mut doc = Document::new(Project::new());
    doc.apply(resolve(
        &Seed::AddTrack(None, true),
        doc.project(),
        &mut next,
    ));
    let before = bytes(doc.project());
    let note = doc.project().notes().next().unwrap().2.id;
    let r = doc.apply(Op::Batch {
        label: "half valid".into(),
        ops: vec![
            Op::SetLyric {
                id: note,
                lyric: "changed".into(),
            },
            Op::RemoveTempo { tick: 0 },
        ],
    });
    assert!(!r.applied);
    assert_eq!(bytes(doc.project()), before);
}

#[test]
fn moving_notes_carries_curves() {
    let mut next = 0;
    let mut doc = Document::new(Project::new());
    doc.apply(resolve(
        &Seed::AddTrack(None, true),
        doc.project(),
        &mut next,
    ));
    let (_, part, n) = doc.project().notes().next().unwrap();
    let (id, abs) = (n.id, part.start + n.start);
    let r = doc.apply(Op::MoveNotes {
        ids: vec![id],
        dt: 480,
        dcents: 0.0,
        carry_curves: true,
    });
    assert!(r.applied, "{:?}", r.diagnostics);
    let curve = &doc.project().tracks[0].curves[&ParamId::new("pitch")];
    // Points under the note [abs, abs + 960) moved by 480 ticks.
    assert_eq!(curve.user_delta.points, vec![(2000 + 480, 10.0)]);
    assert_eq!(curve.model_prediction.points[0], (abs + 480, 1.5));
}

#[test]
fn snapshots_are_isolated_across_threads() {
    let mut next = 0;
    let mut doc = Document::new(Project::new());
    doc.apply(resolve(
        &Seed::AddTrack(None, true),
        doc.project(),
        &mut next,
    ));
    let snap = doc.snapshot();
    let expected = bytes(&snap);
    let reader = std::thread::spawn(move || bytes(&snap));
    doc.apply(Op::SetMeta {
        meta: Meta {
            title: "changed while a reader holds a snapshot".into(),
            ..Meta::default()
        },
    });
    assert_eq!(reader.join().unwrap(), expected);
    assert_ne!(bytes(doc.project()), expected);
}

/// Guards the generator: across many sequences every op kind must be
/// applied successfully many times, and at least 40% of ops must apply.
#[test]
#[cfg_attr(miri, ignore = "slow under Miri; the proptests cover Miri")]
fn generator_reaches_every_op_kind() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    let mut runner = TestRunner::deterministic();
    let strategy = prop::collection::vec(seed(), 30);
    let (mut applied, mut rejected) = (0usize, 0usize);
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for _ in 0..200 {
        let seeds = strategy.new_tree(&mut runner).unwrap().current();
        let (start, mut next) = initial_project(&[]);
        let mut doc = Document::new(start);
        for s in &seeds {
            let op = resolve(s, doc.project(), &mut next);
            let kind = serde_json::to_value(&op).unwrap()["op"]
                .as_str()
                .unwrap()
                .to_owned();
            if doc.apply(op).applied {
                applied += 1;
                *kinds.entry(kind).or_default() += 1;
            } else {
                rejected += 1;
            }
        }
    }
    let expected = [
        "batch",
        "set_meta",
        "set_tempo",
        "remove_tempo",
        "set_time_sig",
        "remove_time_sig",
        "set_key",
        "remove_key",
        "set_seed",
        "select_take",
        "add_track",
        "remove_track",
        "move_track",
        "set_track",
        "add_part",
        "remove_part",
        "move_part",
        "set_part",
        "add_note",
        "remove_notes",
        "move_notes",
        "resize_note",
        "set_lyric",
        "set_phonemes",
        "set_note_expr",
        "set_vocal_modes",
        "set_curve",
        "restore_curve",
    ];
    let rare: Vec<_> = expected
        .iter()
        .filter(|k| kinds.get(**k).copied().unwrap_or(0) < 20)
        .collect();
    assert!(
        rare.is_empty(),
        "applied fewer than 20 times: {rare:?} in {kinds:?}"
    );
    assert!(
        applied * 10 >= (applied + rejected) * 4,
        "applied {applied}, rejected {rejected}"
    );
}
