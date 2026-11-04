//! Applying ops. Every op is validated completely before the project is
//! touched, and returns the op that undoes it.

use crate::diag::{Diagnostic, Rejected};
use crate::ids::{NoteId, Tick, TrackId};
use crate::model::*;
use crate::op::{NotePatch, Op, TrackPatch};
use crate::params::ParamId;
use crate::validate::{note_issues, overlaps, track_issues};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The outcome of a successfully applied op.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ApplyResult {
    /// The op that restores the previous state.
    pub inverse: Op,
    /// Warnings about the new state.
    pub diagnostics: Vec<Diagnostic>,
}

/// Applies `op` to `project`. On error nothing changed.
///
/// Notes on a track must not overlap: an op that adds overlaps to a track
/// is rejected (overlaps a loaded project already had are tolerated, but
/// may not grow).
pub fn apply(project: &mut Project, op: &Op) -> Result<ApplyResult, Rejected> {
    apply_checked(project, op, true)
}

/// [`apply`], optionally without the overlap check (undo and redo return to
/// states that existed, whatever their overlaps).
pub(crate) fn apply_checked(
    project: &mut Project,
    op: &Op,
    check_overlaps: bool,
) -> Result<ApplyResult, Rejected> {
    if !check_overlaps || !may_overlap(op) {
        let inverse = apply_op(project, op)?;
        return Ok(ApplyResult {
            inverse,
            diagnostics: Vec::new(),
        });
    }
    let count = |p: &Project| -> Vec<(TrackId, usize)> {
        p.tracks.iter().map(|t| (t.id, overlaps(t).len())).collect()
    };
    let before = count(project);
    let backup = project.clone();
    let inverse = apply_op(project, op)?;
    let new: Vec<Diagnostic> = project
        .tracks
        .iter()
        .filter(|t| {
            let had = before.iter().find(|b| b.0 == t.id).map_or(0, |b| b.1);
            overlaps(t).len() > had
        })
        .flat_map(overlaps)
        .collect();
    if !new.is_empty() {
        *project = backup;
        return Err(new.into());
    }
    Ok(ApplyResult {
        inverse,
        diagnostics: Vec::new(),
    })
}

/// Whether an op can make notes overlap.
fn may_overlap(op: &Op) -> bool {
    match op {
        Op::Batch { ops, .. } => ops.iter().any(may_overlap),
        Op::AddNotes { .. }
        | Op::MoveNotes { .. }
        | Op::ResizeNotes { .. }
        | Op::UpdateNotes { .. }
        | Op::SplitNote { .. }
        | Op::MergeNotes { .. }
        | Op::AddTrack { .. } => true,
        _ => false,
    }
}

type Res<T = Op> = Result<T, Rejected>;

fn err(code: &str, msg: impl Into<String>) -> Rejected {
    Diagnostic::error(code, msg).into()
}

fn check(ok: bool, code: &str, msg: impl FnOnce() -> String) -> Res<()> {
    if ok { Ok(()) } else { Err(err(code, msg())) }
}

fn track_mut(p: &mut Project, id: TrackId) -> Res<&mut Track> {
    p.track_mut(id)
        .ok_or_else(|| Diagnostic::error("track.not_found", format!("no {id}")).into())
}

fn track_ref(p: &Project, id: TrackId) -> Res<&Track> {
    p.track(id)
        .ok_or_else(|| Diagnostic::error("track.not_found", format!("no {id}")).into())
}

/// Indices of `ids` in the track, which must all exist; duplicates collapse.
fn note_indices(t: &Track, ids: &[NoteId]) -> Res<Vec<usize>> {
    check(!ids.is_empty(), "op.empty", || "no note ids given".into())?;
    let mut idx = Vec::with_capacity(ids.len());
    for &id in ids {
        let i = t.note_index(id).ok_or_else(|| {
            Rejected::from(
                Diagnostic::error("note.not_found", format!("no {id} on {}", t.id))
                    .on_track(t.id)
                    .on_note(id),
            )
        })?;
        idx.push(i);
    }
    idx.sort_unstable();
    idx.dedup();
    Ok(idx)
}

fn validate_note(t: TrackId, n: &Note) -> Res<()> {
    let issues = note_issues(n);
    if issues.is_empty() {
        return Ok(());
    }
    Err(issues
        .into_iter()
        .map(|m| {
            Diagnostic::error("note.invalid", m)
                .on_track(t)
                .on_note(n.id)
        })
        .collect::<Vec<_>>()
        .into())
}

fn sort_notes(t: &mut Track) {
    t.notes.sort_by_key(|n| (n.start, n.id));
}

fn apply_op(p: &mut Project, op: &Op) -> Res {
    match op {
        Op::Batch { label, ops } => {
            // Work on a copy so a failing op leaves `p` untouched.
            let mut scratch = p.clone();
            let mut inverses = ops
                .iter()
                .map(|o| apply_op(&mut scratch, o))
                .collect::<Res<Vec<_>>>()?;
            *p = scratch;
            inverses.reverse();
            Ok(Op::batch(label.clone(), inverses))
        }
        Op::SetMeta { meta } => Ok(Op::SetMeta {
            meta: std::mem::replace(&mut p.meta, meta.clone()),
        }),
        Op::SetTempo { tick, bpm } => {
            check(*tick >= 0, "tempo.invalid", || format!("tick {tick} < 0"))?;
            check(
                bpm.is_finite() && (10.0..=1000.0).contains(bpm),
                "tempo.invalid",
                || format!("bpm {bpm} outside 10..=1000"),
            )?;
            let point = TempoPoint {
                tick: *tick,
                bpm: *bpm,
            };
            match p.tempo.binary_search_by_key(tick, |q| q.tick) {
                Ok(i) => Ok(Op::SetTempo {
                    tick: *tick,
                    bpm: std::mem::replace(&mut p.tempo[i], point).bpm,
                }),
                Err(i) => {
                    p.tempo.insert(i, point);
                    Ok(Op::RemoveTempo { tick: *tick })
                }
            }
        }
        Op::RemoveTempo { tick } => {
            check(*tick != 0, "tempo.first", || {
                "the tempo at tick 0 can be changed but not removed".into()
            })?;
            let i = p
                .tempo
                .binary_search_by_key(tick, |q| q.tick)
                .map_err(|_| err("tempo.not_found", format!("no tempo point at tick {tick}")))?;
            let old = p.tempo.remove(i);
            Ok(Op::SetTempo {
                tick: old.tick,
                bpm: old.bpm,
            })
        }
        Op::SetTimeSig { bar, num, den } => {
            check((1..=32).contains(num), "time_sig.invalid", || {
                format!("numerator {num} outside 1..=32")
            })?;
            check(
                [1, 2, 4, 8, 16, 32].contains(den),
                "time_sig.invalid",
                || format!("denominator {den} is not 1, 2, 4, 8, 16 or 32"),
            )?;
            let point = TimeSigPoint {
                bar: *bar,
                num: *num,
                den: *den,
            };
            match p.time_sig.binary_search_by_key(bar, |s| s.bar) {
                Ok(i) => {
                    let old = std::mem::replace(&mut p.time_sig[i], point);
                    Ok(Op::SetTimeSig {
                        bar: old.bar,
                        num: old.num,
                        den: old.den,
                    })
                }
                Err(i) => {
                    p.time_sig.insert(i, point);
                    Ok(Op::RemoveTimeSig { bar: *bar })
                }
            }
        }
        Op::RemoveTimeSig { bar } => {
            check(*bar != 0, "time_sig.first", || {
                "the time signature at bar 0 can be changed but not removed".into()
            })?;
            let i = p
                .time_sig
                .binary_search_by_key(bar, |s| s.bar)
                .map_err(|_| {
                    err(
                        "time_sig.not_found",
                        format!("no time signature at bar {bar}"),
                    )
                })?;
            let old = p.time_sig.remove(i);
            Ok(Op::SetTimeSig {
                bar: old.bar,
                num: old.num,
                den: old.den,
            })
        }
        Op::SetKey { key } => {
            if let Some(k) = key {
                check(k.tonic < 12, "key.invalid", || {
                    format!("tonic {} outside 0..=11", k.tonic)
                })?;
            }
            Ok(Op::SetKey {
                key: std::mem::replace(&mut p.key, *key),
            })
        }
        Op::SetSections { sections } => {
            for s in sections {
                check(s.start >= 0 && s.start < s.end, "section.invalid", || {
                    format!("section `{}` needs 0 <= start < end", s.name)
                })?;
            }
            let mut sections = sections.clone();
            sections.sort_by(|a, b| (a.start, a.end, &a.name).cmp(&(b.start, b.end, &b.name)));
            Ok(Op::SetSections {
                sections: std::mem::replace(&mut p.sections, sections),
            })
        }
        Op::SetReference { reference } => {
            if let Some(r) = reference {
                check(!r.path.is_empty(), "reference.invalid", || {
                    "reference path is empty".into()
                })?;
                check(r.offset_ms.is_finite(), "reference.invalid", || {
                    "offset_ms must be finite".into()
                })?;
                check(
                    (-60.0..=12.0).contains(&r.gain_db),
                    "reference.invalid",
                    || format!("gain_db {} outside -60..=12", r.gain_db),
                )?;
            }
            Ok(Op::SetReference {
                reference: std::mem::replace(&mut p.reference, reference.clone()),
            })
        }
        Op::AddTrack { track, index } => {
            let mut t = track.clone();
            if t.id.is_unassigned() {
                t.id = TrackId(p.max_track_id().0 + 1);
            }
            check(p.track(t.id).is_none(), "track.exists", || {
                format!("{} already exists", t.id)
            })?;
            let index = index.unwrap_or(p.tracks.len());
            check(index <= p.tracks.len(), "track.index", || {
                format!("index {index} > {} tracks", p.tracks.len())
            })?;
            assign_note_ids(p, &mut t.notes)?;
            let issues = track_issues(&t);
            if !issues.is_empty() {
                return Err(issues.into());
            }
            sort_notes(&mut t);
            canonical_curves(&mut t);
            let id = t.id;
            p.tracks.insert(index, t);
            Ok(Op::RemoveTrack { track: id })
        }
        Op::RemoveTrack { track } => {
            let i = p
                .tracks
                .iter()
                .position(|t| t.id == *track)
                .ok_or_else(|| err("track.not_found", format!("no {track}")))?;
            Ok(Op::AddTrack {
                track: p.tracks.remove(i),
                index: Some(i),
            })
        }
        Op::UpdateTrack { track, patch } => {
            let m = patch.mix.unwrap_or_default();
            check(
                m.gain_db.is_none_or(|g| (-60.0..=12.0).contains(&g)),
                "track.invalid",
                || "gain_db must be in -60..=12".into(),
            )?;
            check(
                m.pan.is_none_or(|x| (-1.0..=1.0).contains(&x)),
                "track.invalid",
                || "pan must be in -1..=1".into(),
            )?;
            let t = track_mut(p, *track)?;
            let inverse = TrackPatch::from_track(t);
            patch.apply_to(t);
            Ok(Op::UpdateTrack {
                track: *track,
                patch: inverse,
            })
        }
        Op::AddNotes { track, notes } => {
            track_ref(p, *track)?;
            let mut notes = notes.clone();
            assign_note_ids(p, &mut notes)?;
            for n in &notes {
                validate_note(*track, n)?;
            }
            let ids = notes.iter().map(|n| n.id).collect();
            let t = track_mut(p, *track)?;
            t.notes.extend(notes);
            sort_notes(t);
            Ok(Op::RemoveNotes { track: *track, ids })
        }
        Op::RemoveNotes { track, ids } => {
            let t = track_mut(p, *track)?;
            let idx = note_indices(t, ids)?;
            let mut removed: Vec<Note> = idx.iter().rev().map(|&i| t.notes.remove(i)).collect();
            removed.reverse();
            Ok(Op::AddNotes {
                track: *track,
                notes: removed,
            })
        }
        Op::UpdateNotes { track, ids, patch } => {
            let t = track_mut(p, *track)?;
            let idx = note_indices(t, ids)?;
            let mut updated = Vec::with_capacity(idx.len());
            for &i in &idx {
                let mut n = t.notes[i].clone();
                patch.apply_to(&mut n);
                validate_note(*track, &n)?;
                updated.push(n);
            }
            let mut inverses = Vec::with_capacity(idx.len());
            for (&i, n) in idx.iter().zip(updated) {
                let old = std::mem::replace(&mut t.notes[i], n);
                inverses.push(Op::UpdateNotes {
                    track: *track,
                    ids: vec![old.id],
                    patch: NotePatch::from_note(&old),
                });
            }
            sort_notes(t);
            Ok(single_or_batch(inverses))
        }
        Op::MoveNotes {
            track,
            ids,
            dticks,
            dkeys,
        } => {
            let t = track_mut(p, *track)?;
            let idx = note_indices(t, ids)?;
            for &i in &idx {
                let n = &t.notes[i];
                check(n.start + dticks >= 0, "note.invalid", || {
                    format!("{} would start before tick 0", n.id)
                })?;
                let k = i32::from(n.key) + dkeys;
                check((0..=127).contains(&k), "note.invalid", || {
                    format!("{} would have key {k}, outside 0..=127", n.id)
                })?;
            }
            for &i in &idx {
                let n = &mut t.notes[i];
                n.start += dticks;
                n.key = (i32::from(n.key) + dkeys) as u8;
            }
            sort_notes(t);
            Ok(Op::MoveNotes {
                track: *track,
                ids: ids.clone(),
                dticks: -dticks,
                dkeys: -dkeys,
            })
        }
        Op::ResizeNotes {
            track,
            ids,
            dlength,
            from_start,
        } => {
            let t = track_mut(p, *track)?;
            let idx = note_indices(t, ids)?;
            for &i in &idx {
                let n = &t.notes[i];
                check(n.length + dlength > 0, "note.invalid", || {
                    format!("{} would have length {}", n.id, n.length + dlength)
                })?;
                check(
                    !from_start || n.start - dlength >= 0,
                    "note.invalid",
                    || format!("{} would start before tick 0", n.id),
                )?;
            }
            for &i in &idx {
                let n = &mut t.notes[i];
                n.length += dlength;
                if *from_start {
                    n.start -= dlength;
                }
            }
            sort_notes(t);
            Ok(Op::ResizeNotes {
                track: *track,
                ids: ids.clone(),
                dlength: -dlength,
                from_start: *from_start,
            })
        }
        Op::SplitNote { track, id, at } => {
            let new_id = NoteId(p.max_note_id().0 + 1);
            let tempo = p.tempo_map();
            let t = track_mut(p, *track)?;
            let [i] = note_indices(t, &[*id])?[..] else {
                unreachable!("one id")
            };
            let old = t.notes[i].clone();
            check(old.start < *at && *at < old.end(), "note.split", || {
                format!(
                    "split point {at} is not strictly inside {} ({}..{})",
                    old.id,
                    old.start,
                    old.end()
                )
            })?;
            let split_ms = (tempo.tick_to_sec(*at) - tempo.tick_to_sec(old.start)) * 1000.0;
            let (left_pts, right_pts): (Vec<_>, Vec<_>) = old
                .pitch
                .points
                .iter()
                .copied()
                .partition(|q| q.ms < split_ms);
            let mut left = old.clone();
            left.length = at - old.start;
            left.pitch.vibrato = None;
            left.pitch.fall = None;
            left.pitch.points = left_pts;
            let right = Note {
                id: new_id,
                start: *at,
                length: old.end() - at,
                lyric: "+".into(),
                alias: None,
                timing: Timing::default(),
                pitch: NotePitch {
                    portamento: Portamento::default(),
                    scoop: None,
                    fall: old.pitch.fall,
                    vibrato: old.pitch.vibrato,
                    points: right_pts
                        .into_iter()
                        .map(|q| PitchPoint {
                            ms: q.ms - split_ms,
                            cents: q.cents,
                        })
                        .collect(),
                },
                ..old.clone()
            };
            t.notes[i] = left;
            t.notes.push(right);
            sort_notes(t);
            Ok(Op::batch(
                "",
                vec![
                    Op::RemoveNotes {
                        track: *track,
                        ids: vec![new_id],
                    },
                    Op::UpdateNotes {
                        track: *track,
                        ids: vec![*id],
                        patch: NotePatch::from_note(&old),
                    },
                ],
            ))
        }
        Op::MergeNotes { track, ids } => {
            let t = track_mut(p, *track)?;
            let idx = note_indices(t, ids)?;
            check(idx.len() >= 2, "op.empty", || {
                "merge_notes needs at least two notes".into()
            })?;
            // Notes are sorted, so the lowest index starts first.
            let end = idx
                .iter()
                .map(|&i| t.notes[i].end())
                .max()
                .expect("non-empty");
            let first = t.notes[idx[0]].clone();
            let mut removed: Vec<Note> =
                idx[1..].iter().rev().map(|&i| t.notes.remove(i)).collect();
            removed.reverse();
            t.notes[idx[0]].length = end - first.start;
            Ok(Op::batch(
                "",
                vec![
                    Op::UpdateNotes {
                        track: *track,
                        ids: vec![first.id],
                        patch: NotePatch::from_note(&first),
                    },
                    Op::AddNotes {
                        track: *track,
                        notes: removed,
                    },
                ],
            ))
        }
        Op::SetLyrics { track, ids, lyrics } => {
            let lyrics = lyrics.to_vec();
            check(lyrics.len() == ids.len(), "lyrics.count", || {
                format!("{} lyrics for {} notes", lyrics.len(), ids.len())
            })?;
            let t = track_mut(p, *track)?;
            note_indices(t, ids)?;
            let mut old = Vec::with_capacity(ids.len());
            for (id, lyric) in ids.iter().zip(lyrics) {
                let n = t.note_mut(*id).expect("checked");
                old.push(std::mem::replace(&mut n.lyric, lyric));
            }
            // Undo in reverse so that repeated ids end with their first value.
            old.reverse();
            Ok(Op::SetLyrics {
                track: *track,
                ids: ids.iter().rev().copied().collect(),
                lyrics: crate::op::Lyrics::List(old),
            })
        }
        Op::SetCurve {
            track,
            param,
            range,
            points,
            interp,
        } => set_curve(p, *track, param, *range, points, *interp),
        Op::ClearCurve {
            track,
            param,
            range,
        } => {
            curve_param(param)?;
            let t = track_mut(p, *track)?;
            let range = match (range, t.curves.get(param)) {
                (Some(r), _) => *r,
                (None, Some(c)) if !c.points.is_empty() => {
                    (c.points[0].0, c.points[c.points.len() - 1].0)
                }
                (None, _) => (0, 0),
            };
            set_curve(p, *track, param, range, &[], None)
        }
        Op::SetEffects { track, effects } => {
            let issues: Vec<Diagnostic> = effects
                .iter()
                .flat_map(|e| e.issues())
                .map(|m| Diagnostic::error("effect.invalid", m).on_track(*track))
                .collect();
            if !issues.is_empty() {
                return Err(issues.into());
            }
            let t = track_mut(p, *track)?;
            Ok(Op::SetEffects {
                track: *track,
                effects: std::mem::replace(&mut t.effects, effects.clone()),
            })
        }
    }
}

fn single_or_batch(mut ops: Vec<Op>) -> Op {
    if ops.len() == 1 {
        ops.pop().expect("one")
    } else {
        Op::batch("", ops)
    }
}

/// Gives every unassigned note a fresh id and checks that the others are
/// unused.
fn assign_note_ids(p: &Project, notes: &mut [Note]) -> Res<()> {
    let mut next = p
        .max_note_id()
        .0
        .max(notes.iter().map(|n| n.id.0).max().unwrap_or(0))
        + 1;
    let mut seen = BTreeSet::new();
    for n in notes {
        if n.id.is_unassigned() {
            n.id = NoteId(next);
            next += 1;
        } else {
            check(p.find_note(n.id).is_none(), "note.exists", || {
                format!("{} already exists", n.id)
            })?;
        }
        check(seen.insert(n.id), "note.exists", || {
            format!("{} given twice", n.id)
        })?;
    }
    Ok(())
}

fn curve_param(param: &ParamId) -> Res<&'static crate::params::ParamDescriptor> {
    param.descriptor().ok_or_else(|| {
        let known: Vec<_> = crate::params::curves().iter().map(|d| d.id).collect();
        err(
            "param.unknown",
            format!("unknown curve `{param}` (known: {})", known.join(", ")),
        )
    })
}

fn set_curve(
    p: &mut Project,
    track: TrackId,
    param: &ParamId,
    (lo, hi): (Tick, Tick),
    points: &[(Tick, f64)],
    interp: Option<Interp>,
) -> Res {
    let d = curve_param(param)?;
    check(lo <= hi, "curve.range", || {
        format!("range {lo}..={hi} is empty")
    })?;
    for &(t, v) in points {
        check((lo..=hi).contains(&t), "curve.range", || {
            format!("point at tick {t} outside range {lo}..={hi}")
        })?;
        check(d.accepts(v), "curve.value", || {
            format!("{param} value {v} outside {}..={} {}", d.min, d.max, d.unit)
        })?;
    }
    let t = track_mut(p, track)?;
    let curve = t.curves.entry(param.clone()).or_default();
    let old: Vec<_> = curve
        .points
        .iter()
        .filter(|q| (lo..=hi).contains(&q.0))
        .copied()
        .collect();
    let old_interp = curve.interp;
    curve.points.retain(|q| !(lo..=hi).contains(&q.0));
    curve.points.extend_from_slice(points);
    if let Some(i) = interp {
        curve.interp = i;
    }
    canonical_curves(t);
    Ok(Op::SetCurve {
        track,
        param: param.clone(),
        range: (lo, hi),
        points: old,
        interp: Some(old_interp),
    })
}

/// Sorts curve points by tick (the last of equal ticks wins) and drops
/// empty curves.
pub(crate) fn canonical_curves(t: &mut Track) {
    for c in t.curves.values_mut() {
        // Stable sort keeps the given order among equal ticks; keep the last.
        c.points.sort_by_key(|q| q.0);
        let mut out: Vec<(Tick, f64)> = Vec::with_capacity(c.points.len());
        for q in c.points.drain(..) {
            match out.last_mut() {
                Some(last) if last.0 == q.0 => *last = q,
                _ => out.push(q),
            }
        }
        c.points = out;
    }
    t.curves.retain(|_, c| !c.points.is_empty());
}
