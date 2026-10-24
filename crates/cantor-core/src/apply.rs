//! Op validation and application.
//!
//! [`apply`] either changes the project and returns the op's inverse, or
//! returns an error diagnostic and leaves the project untouched. Inverses
//! capture previous values rather than negating deltas, so undo restores
//! byte-identical state even for floating-point edits.

use crate::diag::{Diagnostic, DocDiff, EntityRef};
use crate::ids::{NoteId, PartId, Tick, TrackId};
use crate::model::{
    Curve, KeyPoint, LayeredCurve, Note, Project, TempoPoint, TickRange, TimeSigPoint, Track,
    valid_bpm, valid_time_sig, validate_note, validate_track_settings,
};
use crate::op::{NoteExprPatch, Op, TrackPatch};
use crate::params::ParamId;
use std::collections::{BTreeMap, BTreeSet};

type Res<T> = Result<T, Diagnostic>;

/// Applies `op` to `p`. On success returns the inverse op, records changes
/// in `diff` and non-fatal findings in `warnings`. On error `p` is unchanged.
pub(crate) fn apply(
    p: &mut Project,
    op: &Op,
    diff: &mut DocDiff,
    warnings: &mut Vec<Diagnostic>,
) -> Res<Op> {
    match op {
        Op::Batch { label, ops } => {
            let mut inverses = Vec::with_capacity(ops.len());
            for sub in ops {
                match apply(p, sub, diff, warnings) {
                    Ok(inv) => inverses.push(inv),
                    Err(e) => {
                        for inv in inverses.iter().rev() {
                            apply(p, inv, &mut DocDiff::default(), &mut Vec::new())
                                .expect("an inverse op always applies");
                        }
                        return Err(e);
                    }
                }
            }
            inverses.reverse();
            Ok(Op::Batch {
                label: label.clone(),
                ops: inverses,
            })
        }

        Op::SetMeta { meta } => {
            let old = std::mem::replace(&mut p.meta, meta.clone());
            diff.modify(EntityRef::Meta);
            Ok(Op::SetMeta { meta: old })
        }

        Op::SetTempo { tick, bpm } => {
            let (tick, bpm) = (*tick, *bpm);
            if tick < 0 || !valid_bpm(bpm) {
                return Err(Diagnostic::error(
                    "tempo.invalid",
                    format!("tempo {bpm} BPM at tick {tick} is invalid (tick ≥ 0, BPM 1..=1000)"),
                    Some(EntityRef::TempoMap),
                ));
            }
            diff.modify(EntityRef::TempoMap);
            match p.tempo_map.binary_search_by_key(&tick, |t| t.tick) {
                Ok(i) => {
                    let old = std::mem::replace(&mut p.tempo_map[i].bpm, bpm);
                    Ok(Op::SetTempo { tick, bpm: old })
                }
                Err(i) => {
                    p.tempo_map.insert(i, TempoPoint { tick, bpm });
                    Ok(Op::RemoveTempo { tick })
                }
            }
        }
        Op::RemoveTempo { tick } => {
            let tick = *tick;
            let i = p
                .tempo_map
                .binary_search_by_key(&tick, |t| t.tick)
                .ok()
                .filter(|_| tick != 0)
                .ok_or_else(|| {
                    Diagnostic::error(
                        "tempo.not_found",
                        format!(
                            "no removable tempo change at tick {tick} (tick 0 always keeps one)"
                        ),
                        Some(EntityRef::TempoMap),
                    )
                })?;
            let old = p.tempo_map.remove(i);
            diff.modify(EntityRef::TempoMap);
            Ok(Op::SetTempo { tick, bpm: old.bpm })
        }

        Op::SetTimeSig {
            bar,
            numerator,
            denominator,
        } => {
            let (bar, numerator, denominator) = (*bar, *numerator, *denominator);
            if !valid_time_sig(numerator, denominator) {
                return Err(Diagnostic::error(
                    "time_sig.invalid",
                    format!("{numerator}/{denominator} is not a valid time signature"),
                    Some(EntityRef::TimeSigMap),
                ));
            }
            diff.modify(EntityRef::TimeSigMap);
            let new = TimeSigPoint {
                bar,
                numerator,
                denominator,
            };
            match p.time_sig_map.binary_search_by_key(&bar, |t| t.bar) {
                Ok(i) => {
                    let old = std::mem::replace(&mut p.time_sig_map[i], new);
                    Ok(Op::SetTimeSig {
                        bar,
                        numerator: old.numerator,
                        denominator: old.denominator,
                    })
                }
                Err(i) => {
                    p.time_sig_map.insert(i, new);
                    Ok(Op::RemoveTimeSig { bar })
                }
            }
        }
        Op::RemoveTimeSig { bar } => {
            let bar = *bar;
            let i = p
                .time_sig_map
                .binary_search_by_key(&bar, |t| t.bar)
                .ok()
                .filter(|_| bar != 0)
                .ok_or_else(|| {
                    Diagnostic::error(
                        "time_sig.not_found",
                        format!(
                            "no removable time signature at bar {bar} (bar 0 always keeps one)"
                        ),
                        Some(EntityRef::TimeSigMap),
                    )
                })?;
            let old = p.time_sig_map.remove(i);
            diff.modify(EntityRef::TimeSigMap);
            Ok(Op::SetTimeSig {
                bar,
                numerator: old.numerator,
                denominator: old.denominator,
            })
        }

        Op::SetKey { tick, tonic, scale } => {
            let new = KeyPoint {
                tick: *tick,
                tonic: *tonic,
                scale: *scale,
            };
            if new.tick < 0 || new.tonic > 11 {
                return Err(Diagnostic::error(
                    "key.invalid",
                    format!(
                        "key at tick {tick} with tonic {tonic} is invalid (tick ≥ 0, tonic 0..=11)"
                    ),
                    Some(EntityRef::KeyMap),
                ));
            }
            diff.modify(EntityRef::KeyMap);
            match p.key_map.binary_search_by_key(tick, |k| k.tick) {
                Ok(i) => {
                    let old = std::mem::replace(&mut p.key_map[i], new);
                    Ok(Op::SetKey {
                        tick: old.tick,
                        tonic: old.tonic,
                        scale: old.scale,
                    })
                }
                Err(i) => {
                    p.key_map.insert(i, new);
                    Ok(Op::RemoveKey { tick: *tick })
                }
            }
        }
        Op::RemoveKey { tick } => {
            let i = p
                .key_map
                .binary_search_by_key(tick, |k| k.tick)
                .map_err(|_| {
                    Diagnostic::error(
                        "key.not_found",
                        format!("no key change at tick {tick}"),
                        Some(EntityRef::KeyMap),
                    )
                })?;
            let old = p.key_map.remove(i);
            diff.modify(EntityRef::KeyMap);
            Ok(Op::SetKey {
                tick: old.tick,
                tonic: old.tonic,
                scale: old.scale,
            })
        }

        Op::SetSeed { phrase, seed } => {
            let old = match seed {
                Some(s) => p.seeds.insert(*phrase, *s),
                None => p.seeds.remove(phrase),
            };
            diff.modify(EntityRef::Phrase { id: *phrase });
            Ok(Op::SetSeed {
                phrase: *phrase,
                seed: old,
            })
        }
        Op::SelectTake { phrase, take } => {
            let target = Some(EntityRef::Phrase { id: *phrase });
            let old = match (p.takes.get_mut(phrase), take) {
                (Some(lane), Some(t)) => {
                    if !lane.takes.iter().any(|x| x.id == *t) {
                        return Err(Diagnostic::error(
                            "take.not_found",
                            format!("{phrase} has no {t}"),
                            target,
                        ));
                    }
                    lane.selected.replace(*t)
                }
                (Some(lane), None) => lane.selected.take(),
                (None, Some(t)) => {
                    return Err(Diagnostic::error(
                        "take.not_found",
                        format!("{phrase} has no takes, so {t} cannot be selected"),
                        target,
                    ));
                }
                (None, None) => None,
            };
            diff.modify(EntityRef::Phrase { id: *phrase });
            Ok(Op::SelectTake {
                phrase: *phrase,
                take: old,
            })
        }

        Op::AddTrack { index, track } => {
            let index = index.unwrap_or(p.tracks.len());
            if index > p.tracks.len() {
                return Err(Diagnostic::error(
                    "track.invalid_index",
                    format!("index {index} is past the end of {} tracks", p.tracks.len()),
                    None,
                ));
            }
            let mut track = track.clone();
            canonicalize_track(&mut track);
            check_new_ids(p, &track)?;
            check_track_contents(&track)?;
            record_track(diff, &track, DocDiff::add);
            let id = track.id;
            p.tracks.insert(index, track);
            Ok(Op::RemoveTrack { id })
        }
        Op::RemoveTrack { id } => {
            let ti = track_index(p, *id)?;
            let track = p.tracks.remove(ti);
            record_track(diff, &track, DocDiff::remove);
            Ok(Op::AddTrack {
                index: Some(ti),
                track,
            })
        }
        Op::MoveTrack { id, index } => {
            let ti = track_index(p, *id)?;
            if *index >= p.tracks.len() {
                return Err(Diagnostic::error(
                    "track.invalid_index",
                    format!(
                        "index {index} is out of range for {} tracks",
                        p.tracks.len()
                    ),
                    Some(EntityRef::Track { id: *id }),
                ));
            }
            let t = p.tracks.remove(ti);
            p.tracks.insert(*index, t);
            diff.modify(EntityRef::Track { id: *id });
            Ok(Op::MoveTrack { id: *id, index: ti })
        }
        Op::SetTrack { id, patch } => {
            let ti = track_index(p, *id)?;
            let t = &mut p.tracks[ti];
            let mut cand = t.clone();
            cand.parts = Vec::new();
            cand.curves = BTreeMap::new();
            let inverse = apply_track_patch(&mut cand, patch);
            let issues = validate_track_settings(&cand);
            if let Some(issue) = issues.into_iter().next() {
                return Err(Diagnostic::error(
                    "track.invalid",
                    issue,
                    Some(EntityRef::Track { id: *id }),
                ));
            }
            apply_track_patch(t, patch);
            diff.modify(EntityRef::Track { id: *id });
            Ok(Op::SetTrack {
                id: *id,
                patch: inverse,
            })
        }

        Op::AddPart { track, part } => {
            let ti = track_index(p, *track)?;
            let mut part = part.clone();
            part.notes.sort_by_key(|n| (n.start, n.id));
            let probe = Track {
                parts: vec![part.clone()],
                ..empty_track(TrackId(u64::MAX))
            };
            check_new_ids(p, &probe)?;
            check_track_contents(&probe)?;
            diff.add(EntityRef::Part { id: part.id });
            for n in &part.notes {
                diff.add(EntityRef::Note { id: n.id });
            }
            let id = part.id;
            let parts = &mut p.tracks[ti].parts;
            let at = parts.partition_point(|x| (x.start, x.id) < (part.start, part.id));
            parts.insert(at, part);
            Ok(Op::RemovePart { id })
        }
        Op::RemovePart { id } => {
            let (ti, pi) = part_index(p, *id)?;
            let part = p.tracks[ti].parts.remove(pi);
            diff.remove(EntityRef::Part { id: *id });
            for n in &part.notes {
                diff.remove(EntityRef::Note { id: n.id });
            }
            Ok(Op::AddPart {
                track: p.tracks[ti].id,
                part,
            })
        }
        Op::MovePart {
            id,
            dt,
            carry_curves,
        } => {
            let (ti, pi) = part_index(p, *id)?;
            let part = &p.tracks[ti].parts[pi];
            let new_start = part
                .start
                .checked_add(*dt)
                .filter(|s| *s >= 0)
                .ok_or_else(|| {
                    Diagnostic::error(
                        "part.invalid_position",
                        format!("moving {id} by {dt} ticks would start it before tick 0"),
                        Some(EntityRef::Part { id: *id }),
                    )
                })?;
            let src = TickRange {
                start: part.start,
                end: part.start + part.length,
            };
            let mut ops = vec![Op::MovePart {
                id: *id,
                dt: -dt,
                carry_curves: false,
            }];
            if *carry_curves {
                ops.extend(carry_curves_by(&mut p.tracks[ti], &[src], *dt, diff));
            }
            let parts = &mut p.tracks[ti].parts;
            parts[pi].start = new_start;
            parts.sort_by_key(|x| (x.start, x.id));
            diff.modify(EntityRef::Part { id: *id });
            Ok(batch_or_single(op.label(), ops))
        }
        Op::SetPart { id, name, length } => {
            let (ti, pi) = part_index(p, *id)?;
            if length.is_some_and(|l| l < 1) {
                return Err(Diagnostic::error(
                    "part.invalid_length",
                    format!("{id} cannot have length {}", length.unwrap_or_default()),
                    Some(EntityRef::Part { id: *id }),
                ));
            }
            let part = &mut p.tracks[ti].parts[pi];
            let old_name = name
                .as_ref()
                .map(|n| std::mem::replace(&mut part.name, n.clone()));
            let old_length = length.map(|l| std::mem::replace(&mut part.length, l));
            diff.modify(EntityRef::Part { id: *id });
            Ok(Op::SetPart {
                id: *id,
                name: old_name,
                length: old_length,
            })
        }

        Op::AddNote { part, note } => {
            let (ti, pi) = part_index(p, *part)?;
            if p.note(note.id).is_some() {
                return Err(Diagnostic::error(
                    "note.duplicate_id",
                    format!("{} is already in use", note.id),
                    Some(EntityRef::Note { id: note.id }),
                ));
            }
            check_note(note)?;
            let notes = &mut p.tracks[ti].parts[pi].notes;
            let at = notes.partition_point(|n| (n.start, n.id) < (note.start, note.id));
            notes.insert(at, note.clone());
            diff.add(EntityRef::Note { id: note.id });
            warn_overlaps(p, &[note.id], warnings);
            Ok(Op::RemoveNotes { ids: vec![note.id] })
        }
        Op::RemoveNotes { ids } => {
            let locs = note_indices(p, ids)?;
            let mut inverses = Vec::with_capacity(ids.len());
            // Remove from the back so earlier indices stay valid.
            let mut order: Vec<_> = locs.into_iter().collect();
            order.sort_by_key(|&(_, loc)| std::cmp::Reverse(loc));
            for (id, (ti, pi, ni)) in order {
                let note = p.tracks[ti].parts[pi].notes.remove(ni);
                diff.remove(EntityRef::Note { id });
                inverses.push(Op::AddNote {
                    part: p.tracks[ti].parts[pi].id,
                    note,
                });
            }
            Ok(batch_or_single(op.label(), inverses))
        }
        Op::MoveNotes {
            ids,
            dt,
            dcents,
            carry_curves,
        } => {
            let locs = note_indices(p, ids)?;
            if !dcents.is_finite() {
                return Err(Diagnostic::error(
                    "note.invalid",
                    "pitch offset must be finite",
                    None,
                ));
            }
            let mut old = Vec::with_capacity(ids.len());
            let mut moved = Vec::with_capacity(ids.len());
            let mut ranges: BTreeMap<usize, Vec<TickRange>> = BTreeMap::new();
            for (&id, &(ti, pi, ni)) in &locs {
                let part = &p.tracks[ti].parts[pi];
                let n = &part.notes[ni];
                let mut m = n.clone();
                m.start = n.start.saturating_add(*dt);
                m.pitch_cents = n.pitch_cents + dcents;
                check_note(&m)?;
                ranges.entry(ti).or_default().push(TickRange {
                    start: part.start + n.start,
                    end: part.start + n.start + n.length,
                });
                old.push(n.clone());
                moved.push(m);
                diff.modify(EntityRef::Note { id });
            }
            let mut ops = vec![Op::ReplaceNotes { notes: old }];
            if *carry_curves {
                for (ti, rs) in &ranges {
                    ops.extend(carry_curves_by(&mut p.tracks[*ti], rs, *dt, diff));
                }
            }
            replace_notes(p, &locs, moved);
            warn_overlaps(p, ids, warnings);
            Ok(batch_or_single(op.label(), ops))
        }
        Op::ResizeNote { id, length } => {
            let (ti, pi, ni) = note_index(p, *id)?;
            let n = &mut p.tracks[ti].parts[pi].notes[ni];
            if *length < 1 {
                return Err(Diagnostic::error(
                    "note.invalid",
                    format!("{id} cannot have length {length}"),
                    Some(EntityRef::Note { id: *id }),
                ));
            }
            let old = std::mem::replace(&mut n.length, *length);
            diff.modify(EntityRef::Note { id: *id });
            warn_overlaps(p, &[*id], warnings);
            Ok(Op::ResizeNote {
                id: *id,
                length: old,
            })
        }
        Op::SetLyric { id, lyric } => {
            let (ti, pi, ni) = note_index(p, *id)?;
            let n = &mut p.tracks[ti].parts[pi].notes[ni];
            let old = std::mem::replace(&mut n.lyric, lyric.clone());
            diff.modify(EntityRef::Note { id: *id });
            Ok(Op::SetLyric {
                id: *id,
                lyric: old,
            })
        }
        Op::SetPhonemes { id, phonemes } => edit_note(p, *id, diff, |n| {
            let old = std::mem::replace(&mut n.phonemes, phonemes.clone());
            Op::SetPhonemes {
                id: *id,
                phonemes: old,
            }
        }),
        Op::SetNoteExpr { id, expr } => edit_note(p, *id, diff, |n| {
            let e = &mut n.expr;
            let inverse = NoteExprPatch {
                vibrato: expr.vibrato.as_ref().map(|_| e.vibrato.clone()),
                portamento: expr.portamento.as_ref().map(|_| e.portamento.clone()),
                attack: expr.attack.as_ref().map(|_| e.attack.clone()),
                release: expr.release.as_ref().map(|_| e.release.clone()),
            };
            if let Some(v) = &expr.vibrato {
                e.vibrato = v.clone();
            }
            if let Some(v) = &expr.portamento {
                e.portamento = v.clone();
            }
            if let Some(v) = &expr.attack {
                e.attack = v.clone();
            }
            if let Some(v) = &expr.release {
                e.release = v.clone();
            }
            Op::SetNoteExpr {
                id: *id,
                expr: inverse,
            }
        }),
        Op::SetVocalModes { id, modes } => edit_note(p, *id, diff, |n| {
            let old = std::mem::replace(&mut n.modes, modes.clone());
            Op::SetVocalModes {
                id: *id,
                modes: old,
            }
        }),
        Op::ReplaceNotes { notes } => {
            let ids: Vec<NoteId> = notes.iter().map(|n| n.id).collect();
            let locs = note_indices(p, &ids)?;
            for n in notes {
                check_note(n)?;
            }
            let old = locs
                .values()
                .map(|&(ti, pi, ni)| p.tracks[ti].parts[pi].notes[ni].clone())
                .collect();
            for id in &ids {
                diff.modify(EntityRef::Note { id: *id });
            }
            let by_id: BTreeMap<NoteId, Note> = notes.iter().map(|n| (n.id, n.clone())).collect();
            replace_notes(p, &locs, by_id.into_values().collect());
            Ok(Op::ReplaceNotes { notes: old })
        }

        Op::SetCurve {
            track,
            param,
            range,
            points,
            interp,
        } => {
            let ti = track_index(p, *track)?;
            let target = Some(EntityRef::Curve {
                track: *track,
                param: param.clone(),
            });
            let desc = param
                .descriptor()
                .ok_or_else(|| unknown_param(*track, param))?;
            if range.start >= range.end {
                return Err(Diagnostic::error(
                    "curve.invalid_range",
                    format!("range {}..{} is empty", range.start, range.end),
                    target,
                ));
            }
            if let Some(&(t, _)) = points.iter().find(|(t, _)| !range.contains(*t)) {
                return Err(Diagnostic::error(
                    "curve.point_outside_range",
                    format!(
                        "point at tick {t} is outside {}..{}",
                        range.start, range.end
                    ),
                    target,
                ));
            }
            let limit = desc.max_delta();
            if let Some(&(t, v)) = points
                .iter()
                .find(|(_, v)| !v.is_finite() || v.abs() > limit)
            {
                return Err(Diagnostic::error(
                    "curve.value_out_of_range",
                    format!(
                        "`{param}` delta {v} at tick {t} exceeds ±{limit} {}",
                        desc.unit
                    ),
                    target,
                ));
            }
            check_curve_points(points, param, *track)?;
            let curves = &mut p.tracks[ti].curves;
            let old = curves.get(param).cloned();
            let mut c = old.clone().unwrap_or_default();
            let u = &mut c.user_delta;
            u.points.retain(|(t, _)| !range.contains(*t));
            let at = u.points.partition_point(|(t, _)| *t < range.start);
            u.points.splice(at..at, points.iter().copied());
            u.interp = *interp;
            set_curve(curves, *track, param, Some(c), old.is_some(), diff);
            Ok(Op::RestoreCurve {
                track: *track,
                param: param.clone(),
                curve: old,
            })
        }
        Op::RestoreCurve {
            track,
            param,
            curve,
        } => {
            let ti = track_index(p, *track)?;
            if param.descriptor().is_none() {
                return Err(unknown_param(*track, param));
            }
            if let Some(c) = curve {
                check_curve_points(&c.model_prediction.points, param, *track)?;
                check_curve_points(&c.user_delta.points, param, *track)?;
            }
            let curves = &mut p.tracks[ti].curves;
            let old = curves.get(param).cloned();
            set_curve(curves, *track, param, curve.clone(), old.is_some(), diff);
            Ok(Op::RestoreCurve {
                track: *track,
                param: param.clone(),
                curve: old,
            })
        }
    }
}

fn batch_or_single(label: String, mut ops: Vec<Op>) -> Op {
    if ops.len() == 1 {
        ops.pop().expect("one op")
    } else {
        Op::Batch { label, ops }
    }
}

fn empty_track(id: TrackId) -> Track {
    Track {
        id,
        name: String::new(),
        singer: Default::default(),
        backend: Default::default(),
        mix: Default::default(),
        effects: Vec::new(),
        doubling: Default::default(),
        voice_mix: Vec::new(),
        curves: BTreeMap::new(),
        parts: Vec::new(),
    }
}

fn canonicalize_track(t: &mut Track) {
    let mut probe = Project::new();
    probe.tracks.push(std::mem::replace(t, empty_track(t.id)));
    probe.canonicalize();
    *t = probe.tracks.pop().expect("one track");
}

/// Rejects a track whose ids collide with ids already in the project.
fn check_new_ids(p: &Project, t: &Track) -> Res<()> {
    if t.id != TrackId(u64::MAX) && p.track(t.id).is_some() {
        return Err(Diagnostic::error(
            "track.duplicate_id",
            format!("{} is already in use", t.id),
            Some(EntityRef::Track { id: t.id }),
        ));
    }
    let parts: BTreeSet<PartId> = p
        .tracks
        .iter()
        .flat_map(|t| &t.parts)
        .map(|x| x.id)
        .collect();
    let notes: BTreeSet<NoteId> = p.notes().map(|(_, _, n)| n.id).collect();
    for part in &t.parts {
        if parts.contains(&part.id) {
            return Err(Diagnostic::error(
                "part.duplicate_id",
                format!("{} is already in use", part.id),
                Some(EntityRef::Part { id: part.id }),
            ));
        }
        if let Some(n) = part.notes.iter().find(|n| notes.contains(&n.id)) {
            return Err(Diagnostic::error(
                "note.duplicate_id",
                format!("{} is already in use", n.id),
                Some(EntityRef::Note { id: n.id }),
            ));
        }
    }
    Ok(())
}

/// Validates a track's contents on their own, reporting the first error.
fn check_track_contents(t: &Track) -> Res<()> {
    let mut probe = Project::new();
    probe.tracks.push(t.clone());
    match probe.validate().into_iter().find(Diagnostic::is_error) {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

fn record_track(diff: &mut DocDiff, t: &Track, f: fn(&mut DocDiff, EntityRef)) {
    f(diff, EntityRef::Track { id: t.id });
    for param in t.curves.keys() {
        f(
            diff,
            EntityRef::Curve {
                track: t.id,
                param: param.clone(),
            },
        );
    }
    for part in &t.parts {
        f(diff, EntityRef::Part { id: part.id });
        for n in &part.notes {
            f(diff, EntityRef::Note { id: n.id });
        }
    }
}

/// Applies a patch and returns the patch that restores the old values.
fn apply_track_patch(t: &mut Track, patch: &TrackPatch) -> TrackPatch {
    fn swap<T: Clone>(field: &mut T, new: &Option<T>) -> Option<T> {
        new.as_ref().map(|v| std::mem::replace(field, v.clone()))
    }
    TrackPatch {
        name: swap(&mut t.name, &patch.name),
        singer: swap(&mut t.singer, &patch.singer),
        backend: swap(&mut t.backend, &patch.backend),
        mix: swap(&mut t.mix, &patch.mix),
        effects: swap(&mut t.effects, &patch.effects),
        doubling: swap(&mut t.doubling, &patch.doubling),
        voice_mix: swap(&mut t.voice_mix, &patch.voice_mix),
    }
}

fn track_index(p: &Project, id: TrackId) -> Res<usize> {
    p.tracks.iter().position(|t| t.id == id).ok_or_else(|| {
        Diagnostic::error(
            "track.not_found",
            format!("{id} does not exist"),
            Some(EntityRef::Track { id }),
        )
    })
}

fn part_index(p: &Project, id: PartId) -> Res<(usize, usize)> {
    p.tracks
        .iter()
        .enumerate()
        .find_map(|(ti, t)| t.parts.iter().position(|x| x.id == id).map(|pi| (ti, pi)))
        .ok_or_else(|| {
            Diagnostic::error(
                "part.not_found",
                format!("{id} does not exist"),
                Some(EntityRef::Part { id }),
            )
        })
}

type NoteLoc = (usize, usize, usize);

fn note_index(p: &Project, id: NoteId) -> Res<NoteLoc> {
    for (ti, t) in p.tracks.iter().enumerate() {
        for (pi, part) in t.parts.iter().enumerate() {
            if let Some(ni) = part.notes.iter().position(|n| n.id == id) {
                return Ok((ti, pi, ni));
            }
        }
    }
    Err(Diagnostic::error(
        "note.not_found",
        format!("{id} does not exist"),
        Some(EntityRef::Note { id }),
    ))
}

/// Locates several distinct notes.
fn note_indices(p: &Project, ids: &[NoteId]) -> Res<BTreeMap<NoteId, NoteLoc>> {
    if ids.is_empty() {
        return Err(Diagnostic::error(
            "note.empty_selection",
            "no notes given",
            None,
        ));
    }
    let mut out = BTreeMap::new();
    for &id in ids {
        if out.insert(id, note_index(p, id)?).is_some() {
            return Err(Diagnostic::error(
                "note.duplicate_id",
                format!("{id} is listed twice"),
                Some(EntityRef::Note { id }),
            ));
        }
    }
    Ok(out)
}

fn check_note(n: &Note) -> Res<()> {
    match validate_note(n) {
        Some(msg) => Err(Diagnostic::error(
            "note.invalid",
            msg,
            Some(EntityRef::Note { id: n.id }),
        )),
        None => Ok(()),
    }
}

/// Edits one note in place through `f`, which returns the inverse op. The
/// edit is rejected if it leaves the note invalid.
fn edit_note(
    p: &mut Project,
    id: NoteId,
    diff: &mut DocDiff,
    f: impl FnOnce(&mut Note) -> Op,
) -> Res<Op> {
    let (ti, pi, ni) = note_index(p, id)?;
    let n = &mut p.tracks[ti].parts[pi].notes[ni];
    let mut cand = n.clone();
    let inverse = f(&mut cand);
    check_note(&cand)?;
    *n = cand;
    diff.modify(EntityRef::Note { id });
    Ok(inverse)
}

/// Writes notes back to their (track, part) slots, then restores note order
/// in every touched part. `new` must hold one note per entry of `locs`.
fn replace_notes(p: &mut Project, locs: &BTreeMap<NoteId, NoteLoc>, new: Vec<Note>) {
    let mut touched = BTreeSet::new();
    for n in new {
        let (ti, pi, ni) = locs[&n.id];
        p.tracks[ti].parts[pi].notes[ni] = n;
        touched.insert((ti, pi));
    }
    for (ti, pi) in touched {
        p.tracks[ti].parts[pi]
            .notes
            .sort_by_key(|n| (n.start, n.id));
    }
}

/// Moves the curve points inside `ranges` by `dt`. Points already in the
/// destination ranges are replaced. Returns ops restoring changed curves.
fn carry_curves_by(t: &mut Track, ranges: &[TickRange], dt: i64, diff: &mut DocDiff) -> Vec<Op> {
    if dt == 0 {
        return Vec::new();
    }
    let inside = |rs: &[TickRange], x: Tick| rs.iter().any(|r| r.contains(x));
    let dst: Vec<TickRange> = ranges
        .iter()
        .map(|r| TickRange {
            start: r.start + dt,
            end: r.end + dt,
        })
        .collect();
    let mut restores = Vec::new();
    for (param, curve) in t.curves.iter_mut() {
        let touches = |c: &Curve| {
            c.points
                .iter()
                .any(|&(x, _)| inside(ranges, x) || inside(&dst, x))
        };
        if !touches(&curve.model_prediction) && !touches(&curve.user_delta) {
            continue;
        }
        restores.push(Op::RestoreCurve {
            track: t.id,
            param: param.clone(),
            curve: Some(curve.clone()),
        });
        for layer in [&mut curve.model_prediction, &mut curve.user_delta] {
            let mut moved = Vec::new();
            layer.points.retain(|&(x, v)| {
                if inside(ranges, x) {
                    moved.push((x + dt, v));
                    false
                } else {
                    true
                }
            });
            layer.points.retain(|&(x, _)| !inside(&dst, x));
            layer.points.extend(moved);
            layer.points.sort_by_key(|p| p.0);
        }
        diff.modify(EntityRef::Curve {
            track: t.id,
            param: param.clone(),
        });
    }
    // A curve whose only points sat in the destination is now empty;
    // canonical form drops it.
    let track = t.id;
    t.curves.retain(|param, c| {
        let keep = !c.is_empty();
        if !keep {
            diff.remove(EntityRef::Curve {
                track,
                param: param.clone(),
            });
        }
        keep
    });
    restores
}

fn check_curve_points(points: &[(Tick, f64)], param: &ParamId, track: TrackId) -> Res<()> {
    let sorted = points.windows(2).all(|w| w[0].0 < w[1].0);
    let finite = points.iter().all(|p| p.1.is_finite());
    if sorted && finite {
        return Ok(());
    }
    Err(Diagnostic::error(
        "curve.invalid_points",
        format!("`{param}` points must have strictly increasing ticks and finite values"),
        Some(EntityRef::Curve {
            track,
            param: param.clone(),
        }),
    ))
}

fn unknown_param(track: TrackId, param: &ParamId) -> Diagnostic {
    Diagnostic::error(
        "curve.unknown_param",
        format!("`{param}` is not a known parameter; see the parameter registry"),
        Some(EntityRef::Curve {
            track,
            param: param.clone(),
        }),
    )
}

/// Stores or removes a curve, keeping canonical form (no empty curves).
fn set_curve(
    curves: &mut BTreeMap<ParamId, LayeredCurve>,
    track: TrackId,
    param: &ParamId,
    curve: Option<LayeredCurve>,
    existed: bool,
    diff: &mut DocDiff,
) {
    let e = EntityRef::Curve {
        track,
        param: param.clone(),
    };
    match curve.filter(|c| !c.is_empty()) {
        Some(c) => {
            curves.insert(param.clone(), c);
            if existed { diff.modify(e) } else { diff.add(e) }
        }
        None => {
            if curves.remove(param).is_some() {
                diff.remove(e);
            }
        }
    }
}

/// Warns about notes that now overlap a neighbor in their part.
fn warn_overlaps(p: &Project, ids: &[NoteId], warnings: &mut Vec<Diagnostic>) {
    for &id in ids {
        let Ok((ti, pi, ni)) = note_index(p, id) else {
            continue;
        };
        let notes = &p.tracks[ti].parts[pi].notes;
        let n = &notes[ni];
        let overlaps_prev = ni > 0 && notes[ni - 1].start + notes[ni - 1].length > n.start;
        let overlaps_next = notes
            .get(ni + 1)
            .is_some_and(|m| n.start + n.length > m.start);
        if overlaps_prev || overlaps_next {
            warnings.push(Diagnostic::warning(
                "note.overlap",
                format!("{id} overlaps a neighboring note; the track renders monophonically"),
                Some(EntityRef::Note { id }),
            ));
        }
    }
}
