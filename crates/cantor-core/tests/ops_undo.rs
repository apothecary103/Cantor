//! Property tests for ops and history.
//!
//! Random op sequences (valid and invalid) are generated as abstract
//! seeds and bound to the ids of the current project, so most ops address
//! real notes while some deliberately miss. Properties: applying any
//! sequence and undoing everything restores an identical project; redoing
//! everything reaches the same final state; ops survive a JSON round trip;
//! projects stay canonical.

use cantor_core::*;
use proptest::prelude::*;

#[derive(Clone, Debug)]
enum Seed {
    Meta(String),
    Tempo(i64, f64),
    RemoveTempo(usize),
    TimeSig(u32, u8, u8),
    RemoveTimeSig(usize),
    Key(Option<(u8, usize)>),
    Sections(Vec<(i64, i64)>),
    Reference(Option<f64>),
    AddTrack(Option<usize>, Vec<(i64, i64, u8)>),
    RemoveTrack(usize),
    UpdateTrack(usize, Option<String>, Option<f64>, Option<bool>),
    AddNotes(usize, Vec<(i64, i64, u8, f64)>),
    RemoveNotes(usize, Vec<usize>),
    UpdateNotes(usize, Vec<usize>, u8),
    MoveNotes(usize, Vec<usize>, i64, i32),
    ResizeNotes(usize, Vec<usize>, i64, bool),
    Split(usize, usize, i64),
    Merge(usize, Vec<usize>),
    Lyrics(usize, Vec<usize>, bool),
    SetCurve(usize, usize, (i64, i64), Vec<(i64, f64)>, Option<usize>),
    ClearCurve(usize, usize, Option<(i64, i64)>),
    Effects(usize, Vec<(usize, f64)>),
    Batch(Vec<Seed>),
}

const PARAMS: &[&str] = &["pitd", "dyn", "gen", "bre", "bri", "tone", "nope"];
const LYRICS: &[&str] = &["a", "ka", "+", "-", "br", "あ", ""];
const INTERPS: &[Interp] = &[Interp::Linear, Interp::Step, Interp::Smooth];
const SCALES: &[Scale] = &[Scale::Major, Scale::HarmonicMinor, Scale::MinorPentatonic];
const EFFECTS: &[EffectKind] = &[
    EffectKind::Eq,
    EffectKind::Compressor,
    EffectKind::DeEsser,
    EffectKind::Reverb,
    EffectKind::Delay,
];

fn leaf() -> impl Strategy<Value = Seed> {
    let i = || 0usize..16;
    let ids = || prop::collection::vec(0usize..40, 0..4);
    let tick = || -100i64..20_000;
    prop_oneof![
        "[a-z]{0,6}".prop_map(Seed::Meta),
        (tick(), 5.0f64..1100.0).prop_map(|(t, b)| Seed::Tempo(t, b)),
        i().prop_map(Seed::RemoveTempo),
        (0u32..8, 0u8..34, 0u8..40).prop_map(|(b, n, d)| Seed::TimeSig(b, n, d)),
        i().prop_map(Seed::RemoveTimeSig),
        prop::option::of((0u8..14, i())).prop_map(Seed::Key),
        prop::collection::vec((tick(), tick()), 0..3).prop_map(Seed::Sections),
        prop::option::of(-80.0f64..20.0).prop_map(Seed::Reference),
        (
            prop::option::of(0usize..4),
            prop::collection::vec((0i64..5000, -10i64..1000, 0u8..130), 0..4)
        )
            .prop_map(|(a, b)| Seed::AddTrack(a, b)),
        i().prop_map(Seed::RemoveTrack),
        (
            i(),
            prop::option::of("[a-z]{1,4}"),
            prop::option::of(-80.0f64..20.0),
            prop::option::of(any::<bool>())
        )
            .prop_map(|(t, n, g, m)| Seed::UpdateTrack(t, n, g, m)),
        (
            i(),
            prop::collection::vec((tick(), -10i64..2000, 0u8..130, -10.0f64..210.0), 1..5)
        )
            .prop_map(|(t, n)| Seed::AddNotes(t, n)),
        (i(), ids()).prop_map(|(t, n)| Seed::RemoveNotes(t, n)),
        (i(), ids(), any::<u8>()).prop_map(|(t, n, k)| Seed::UpdateNotes(t, n, k)),
        (i(), ids(), -3000i64..3000, -20i32..20)
            .prop_map(|(t, n, d, k)| Seed::MoveNotes(t, n, d, k)),
        (i(), ids(), -1000i64..1000, any::<bool>())
            .prop_map(|(t, n, d, f)| Seed::ResizeNotes(t, n, d, f)),
        (i(), 0usize..40, -50i64..2000).prop_map(|(t, n, a)| Seed::Split(t, n, a)),
        (i(), ids()).prop_map(|(t, n)| Seed::Merge(t, n)),
        (i(), ids(), any::<bool>()).prop_map(|(t, n, s)| Seed::Lyrics(t, n, s)),
        (
            i(),
            0usize..PARAMS.len(),
            (tick(), tick()),
            prop::collection::vec((tick(), -1500.0f64..1500.0), 0..5),
            prop::option::of(0usize..3)
        )
            .prop_map(|(t, p, r, pts, ip)| Seed::SetCurve(t, p, r, pts, ip)),
        (
            i(),
            0usize..PARAMS.len(),
            prop::option::of((tick(), tick()))
        )
            .prop_map(|(t, p, r)| Seed::ClearCurve(t, p, r)),
        (i(), prop::collection::vec((0usize..5, -0.5f64..1.5), 0..3))
            .prop_map(|(t, e)| Seed::Effects(t, e)),
    ]
}

fn seed() -> impl Strategy<Value = Seed> {
    leaf().prop_recursive(2, 12, 4, |inner| {
        prop::collection::vec(inner, 0..4).prop_map(Seed::Batch)
    })
}

fn pick<T: Copy>(v: &[T], i: usize) -> Option<T> {
    (!v.is_empty()).then(|| v[i % v.len()])
}

/// Binds a seed to the project: indices pick existing ids, with some
/// chance of a missing one.
fn bind(p: &Project, s: &Seed) -> Op {
    let track = |i: usize| {
        if i == 15 {
            TrackId(999)
        } else {
            pick(&p.tracks.iter().map(|t| t.id).collect::<Vec<_>>(), i).unwrap_or(TrackId(999))
        }
    };
    let note_ids = |t: TrackId, idx: &[usize]| -> Vec<NoteId> {
        let all: Vec<NoteId> = p
            .track(t)
            .map(|t| t.notes.iter().map(|n| n.id).collect())
            .unwrap_or_default();
        idx.iter()
            .map(|&i| {
                if i == 39 {
                    NoteId(9999)
                } else {
                    pick(&all, i).unwrap_or(NoteId(9999))
                }
            })
            .collect()
    };
    match s.clone() {
        Seed::Meta(title) => Op::SetMeta {
            meta: Meta {
                title,
                ..Meta::default()
            },
        },
        Seed::Tempo(tick, bpm) => Op::SetTempo { tick, bpm },
        Seed::RemoveTempo(i) => Op::RemoveTempo {
            tick: pick(&p.tempo, i).map_or(5, |t| t.tick),
        },
        Seed::TimeSig(bar, num, den) => Op::SetTimeSig { bar, num, den },
        Seed::RemoveTimeSig(i) => Op::RemoveTimeSig {
            bar: pick(&p.time_sig, i).map_or(3, |s| s.bar),
        },
        Seed::Key(k) => Op::SetKey {
            key: k.map(|(tonic, s)| Key {
                tonic,
                scale: SCALES[s % SCALES.len()],
            }),
        },
        Seed::Sections(v) => Op::SetSections {
            sections: v
                .into_iter()
                .map(|(start, end)| Section {
                    name: format!("s{start}"),
                    start,
                    end,
                })
                .collect(),
        },
        Seed::Reference(g) => Op::SetReference {
            reference: g.map(|gain_db| Reference {
                path: "vocal.wav".into(),
                offset_ms: 12.5,
                gain_db,
                muted: false,
            }),
        },
        Seed::AddTrack(index, notes) => Op::AddTrack {
            track: Track {
                name: "t".into(),
                notes: notes
                    .into_iter()
                    .map(|(s, l, k)| Note::new(s, l, k, "la"))
                    .collect(),
                ..Track::default()
            },
            index,
        },
        Seed::RemoveTrack(i) => Op::RemoveTrack { track: track(i) },
        Seed::UpdateTrack(i, name, gain_db, mute) => Op::UpdateTrack {
            track: track(i),
            patch: TrackPatch {
                name,
                mix: Some(MixPatch {
                    gain_db,
                    mute,
                    ..MixPatch::default()
                }),
                ..TrackPatch::default()
            },
        },
        Seed::AddNotes(i, notes) => Op::AddNotes {
            track: track(i),
            notes: notes
                .into_iter()
                .map(|(s, l, k, vel)| Note {
                    vel,
                    ..Note::new(s, l, k, "a")
                })
                .collect(),
        },
        Seed::RemoveNotes(i, n) => {
            let t = track(i);
            Op::RemoveNotes {
                track: t,
                ids: note_ids(t, &n),
            }
        }
        Seed::UpdateNotes(i, n, k) => {
            let t = track(i);
            let patch = match k % 6 {
                0 => NotePatch {
                    key: Some(k),
                    lyric: Some("o".into()),
                    ..NotePatch::default()
                },
                1 => NotePatch {
                    alias: Some(if k % 2 == 0 {
                        None
                    } else {
                        Some("- ka".into())
                    }),
                    vol: Some(f64::from(k)),
                    ..NotePatch::default()
                },
                2 => NotePatch {
                    pitch: Some(PitchPatch {
                        vibrato: Some(if k > 128 {
                            None
                        } else {
                            Some(VibratoPatch {
                                depth_cents: Some(f64::from(k)),
                                ..VibratoPatch::default()
                            })
                        }),
                        scoop: Some(Some(Bend {
                            cents: -f64::from(k),
                            length_ms: 80.0,
                        })),
                        ..PitchPatch::default()
                    }),
                    ..NotePatch::default()
                },
                3 => NotePatch {
                    start: Some(i64::from(k) * 37 - 100),
                    length: Some(i64::from(k) * 3),
                    ..NotePatch::default()
                },
                4 => NotePatch {
                    timing: Some(TimingPatch {
                        preutter_ms: Some(Some(f64::from(k))),
                        offset_ms: Some(1.5),
                        ..TimingPatch::default()
                    }),
                    pitch: Some(PitchPatch {
                        portamento: Some(PortamentoPatch {
                            length_ms: Some(f64::from(k) * 5.0),
                            shape: Some(Shape::Linear),
                            ..PortamentoPatch::default()
                        }),
                        points: Some(vec![PitchPoint {
                            ms: 10.0,
                            cents: f64::from(k),
                        }]),
                        ..PitchPatch::default()
                    }),
                    ..NotePatch::default()
                },
                _ => NotePatch::default(),
            };
            Op::UpdateNotes {
                track: t,
                ids: note_ids(t, &n),
                patch,
            }
        }
        Seed::MoveNotes(i, n, dticks, dkeys) => {
            let t = track(i);
            Op::MoveNotes {
                track: t,
                ids: note_ids(t, &n),
                dticks,
                dkeys,
            }
        }
        Seed::ResizeNotes(i, n, dlength, from_start) => {
            let t = track(i);
            Op::ResizeNotes {
                track: t,
                ids: note_ids(t, &n),
                dlength,
                from_start,
            }
        }
        Seed::Split(i, n, off) => {
            let t = track(i);
            let id = note_ids(t, &[n])[0];
            let at = p.find_note(id).map_or(0, |(_, n)| n.start) + off;
            Op::SplitNote { track: t, id, at }
        }
        Seed::Merge(i, n) => {
            let t = track(i);
            Op::MergeNotes {
                track: t,
                ids: note_ids(t, &n),
            }
        }
        Seed::Lyrics(i, n, as_text) => {
            let t = track(i);
            let ids = note_ids(t, &n);
            let words: Vec<String> = (0..ids.len())
                .map(|j| LYRICS[(j + n.len()) % (LYRICS.len() - 1)].to_string())
                .collect();
            Op::SetLyrics {
                track: t,
                ids,
                lyrics: if as_text {
                    Lyrics::Text(words.join(" "))
                } else {
                    Lyrics::List(words)
                },
            }
        }
        Seed::SetCurve(i, param, range, points, interp) => Op::SetCurve {
            track: track(i),
            param: ParamId::new(PARAMS[param]),
            range,
            points,
            interp: interp.map(|j| INTERPS[j]),
        },
        Seed::ClearCurve(i, param, range) => Op::ClearCurve {
            track: track(i),
            param: ParamId::new(PARAMS[param]),
            range,
        },
        Seed::Effects(i, e) => Op::SetEffects {
            track: track(i),
            effects: e
                .into_iter()
                .map(|(k, mix)| {
                    let mut slot = EffectSlot::new(EFFECTS[k]);
                    if EFFECTS[k] == EffectKind::Reverb || EFFECTS[k] == EffectKind::Delay {
                        slot.params.insert("mix".into(), mix);
                    }
                    slot
                })
                .collect(),
        },
        Seed::Batch(seeds) => {
            // Bind against the state each op will see; ops that fail are
            // still included (and make the batch fail).
            let mut scratch = p.clone();
            let mut ops = Vec::new();
            for s in &seeds {
                let op = bind(&scratch, s);
                let _ = apply(&mut scratch, &op);
                ops.push(op);
            }
            Op::batch("batch", ops)
        }
    }
}

fn start_project() -> Project {
    let mut p = Project::default();
    for (k, name) in ["lead", "harmony"].into_iter().enumerate() {
        let notes = (0..6)
            .map(|i| Note::new(i * 480, 480, 60 + k as u8 + i as u8, LYRICS[i as usize]))
            .collect();
        apply(
            &mut p,
            &Op::AddTrack {
                track: Track {
                    name: name.into(),
                    notes,
                    ..Track::default()
                },
                index: None,
            },
        )
        .unwrap();
    }
    p
}

fn canonical(p: &Project) -> bool {
    let mut c = p.clone();
    c.canonicalize();
    c == *p
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: if cfg!(miri) { 4 } else { 256 },
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn undo_restores_and_redo_replays(seeds in prop::collection::vec(seed(), 1..24)) {
        let start = start_project();
        let mut doc = Document::new(start.clone());
        let mut applied = 0;
        for s in &seeds {
            let op = bind(doc.project(), s);
            let json = serde_json::to_string(&op).unwrap();
            prop_assert_eq!(&serde_json::from_str::<Op>(&json).unwrap(), &op, "{}", json);
            let before = doc.project().clone();
            match doc.apply(op) {
                Ok(_) => applied += 1,
                Err(e) => {
                    prop_assert!(!e.diagnostics.is_empty());
                    prop_assert_eq!(doc.project(), &before, "rejected op changed the project");
                }
            }
            prop_assert!(canonical(doc.project()));
            let errors: Vec<_> = doc.project().validate().into_iter().filter(Diagnostic::is_error).collect();
            prop_assert!(errors.is_empty(), "{:?}", errors);
        }
        let end = doc.project().clone();
        for _ in 0..applied {
            prop_assert!(doc.undo().unwrap().is_some());
        }
        prop_assert!(!doc.can_undo());
        prop_assert_eq!(doc.project(), &start);
        for _ in 0..applied {
            prop_assert!(doc.redo().unwrap().is_some());
        }
        prop_assert_eq!(doc.project(), &end);

        // Checkpoints: restore the start and undo the restore.
        doc.checkpoint("end");
        let mut fresh = Document::new(start.clone());
        fresh.checkpoint("start");
        fresh.replace(end.clone(), "import").unwrap();
        prop_assert_eq!(fresh.project(), &end);
        fresh.restore("start").unwrap();
        prop_assert_eq!(fresh.project(), &start);
        fresh.undo().unwrap();
        prop_assert_eq!(fresh.project(), &end);
    }
}

#[test]
fn minimal_json_deserializes() {
    let add = |start| {
        serde_json::from_str::<Op>(&format!(
            r#"{{"op": "add_notes", "track": 1, "notes": [{{"start": {start}, "length": 480, "key": 60, "lyric": "a"}}]}}"#
        ))
        .unwrap()
    };
    let mut p = start_project();
    // Overlapping the existing first note is rejected.
    let e = apply(&mut p, &add(0)).unwrap_err();
    assert_eq!(e.diagnostics[0].code, "note.overlap");
    let json = serde_json::to_value(&e.diagnostics[0]).unwrap();
    assert_eq!(json["level"], "error");
    assert_eq!(json["entity"]["kind"], "note");
    assert_eq!(p, start_project());
    let r = apply(&mut p, &add(4800)).unwrap();
    let Op::RemoveNotes { ids, .. } = r.inverse else {
        panic!("{:?}", r.inverse)
    };
    assert_eq!(ids, vec![NoteId(13)]);
    let n = p.find_note(NoteId(13)).unwrap().1;
    assert_eq!(
        (n.vel, n.vol, n.pitch.portamento.length_ms),
        (100.0, 100.0, 80.0)
    );
    let json = serde_json::to_value(n).unwrap();
    assert!(json.get("mod").is_none() && json.get("mod_").is_none());

    let p: Project = serde_json::from_str(r#"{"tracks": [{"id": 1, "notes": []}]}"#).unwrap();
    assert_eq!(
        p.tempo,
        vec![TempoPoint {
            tick: 0,
            bpm: 120.0
        }]
    );
    assert!(
        serde_json::from_str::<Op>(r#"{"op": "set_tempo", "tick": 0, "bpm": 90, "x": 1}"#).is_err(),
        "unknown fields are rejected"
    );
}

#[test]
fn patches_merge_and_clear() {
    let mut p = start_project();
    let id = p.tracks[0].notes[1].id;
    let patch: NotePatch = serde_json::from_str(
        r#"{"pitch": {"vibrato": {"depth_cents": 35}, "portamento": {"length_ms": 120}}, "alias": "- ka"}"#,
    )
    .unwrap();
    let upd = |patch| Op::UpdateNotes {
        track: TrackId(1),
        ids: vec![id],
        patch,
    };
    apply(&mut p, &upd(patch)).unwrap();
    let n = p.find_note(id).unwrap().1.clone();
    let v = n.pitch.vibrato.unwrap();
    assert_eq!(
        (v.depth_cents, v.period_ms),
        (35.0, Vibrato::default().period_ms)
    );
    assert_eq!(n.pitch.portamento.length_ms, 120.0);
    assert_eq!(n.pitch.portamento.start_ms, -40.0);
    assert_eq!(n.alias.as_deref(), Some("- ka"));

    let clear: NotePatch =
        serde_json::from_str(r#"{"pitch": {"vibrato": null}, "alias": null}"#).unwrap();
    apply(&mut p, &upd(clear)).unwrap();
    let n = p.find_note(id).unwrap().1;
    assert!(n.pitch.vibrato.is_none() && n.alias.is_none());

    let mix: TrackPatch = serde_json::from_str(r#"{"mix": {"mute": true}}"#).unwrap();
    apply(
        &mut p,
        &Op::UpdateTrack {
            track: TrackId(1),
            patch: mix,
        },
    )
    .unwrap();
    assert!(p.tracks[0].mix.mute && p.tracks[0].mix.gain_db == 0.0);

    let bad: NotePatch = serde_json::from_str(r#"{"vel": 500}"#).unwrap();
    let e = apply(&mut p, &upd(bad)).unwrap_err();
    assert_eq!(e.diagnostics[0].code, "note.invalid");
}

#[test]
fn split_and_merge() {
    let mut p = start_project();
    let t = TrackId(1);
    let id = p.tracks[0].notes[2].id;
    let start = p.tracks[0].notes[2].start;
    let before = p.clone();
    let r = apply(
        &mut p,
        &Op::SplitNote {
            track: t,
            id,
            at: start + 120,
        },
    )
    .unwrap();
    let notes = &p.tracks[0].notes;
    assert_eq!(notes.len(), 7);
    assert_eq!(
        (notes[2].length, notes[3].start, notes[3].length),
        (120, start + 120, 360)
    );
    assert_eq!(notes[3].lyric, "+");
    let ids = vec![notes[2].id, notes[3].id];
    let mut merged = p.clone();
    apply(&mut merged, &Op::MergeNotes { track: t, ids }).unwrap();
    assert_eq!(merged.tracks[0].notes[2], before.tracks[0].notes[2]);
    apply(&mut p, &r.inverse).unwrap();
    assert_eq!(p, before);
}
