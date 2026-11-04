//! Canonical form and validation of whole projects.

use crate::apply::canonical_curves;
use crate::diag::Diagnostic;
use crate::model::*;
use crate::params::note_param;
use std::collections::BTreeSet;

impl Project {
    /// Puts the project in canonical form: tempo, time signatures and
    /// sections sorted (first tempo at tick 0, first signature at bar 0),
    /// notes sorted by (start, id), curve points sorted with unique ticks,
    /// empty curves removed. Ops keep projects canonical; call this after
    /// loading or building a project by hand.
    pub fn canonicalize(&mut self) {
        self.format = FORMAT;
        self.tempo.sort_by_key(|p| p.tick);
        dedup_last(&mut self.tempo, |p| p.tick);
        match self.tempo.first() {
            None => self.tempo.push(TempoPoint::default()),
            Some(p) if p.tick != 0 => self.tempo.insert(0, TempoPoint { tick: 0, ..*p }),
            _ => {}
        }
        self.time_sig.sort_by_key(|s| s.bar);
        dedup_last(&mut self.time_sig, |s| s.bar);
        match self.time_sig.first() {
            None => self.time_sig.push(TimeSigPoint::default()),
            Some(s) if s.bar != 0 => self.time_sig.insert(0, TimeSigPoint { bar: 0, ..*s }),
            _ => {}
        }
        self.sections
            .sort_by(|a, b| (a.start, a.end, &a.name).cmp(&(b.start, b.end, &b.name)));
        for t in &mut self.tracks {
            t.notes.sort_by_key(|n| (n.start, n.id));
            canonical_curves(t);
        }
    }

    /// Everything wrong: out-of-range values, duplicate ids, unknown
    /// curves, unsorted data, overlapping notes.
    pub fn validate(&self) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut err = |code: &str, msg: String| out.push(Diagnostic::error(code, msg));
        if self.format != FORMAT {
            err(
                "project.format",
                format!("format {} is not {FORMAT}", self.format),
            );
        }
        if self.tempo.first().is_none_or(|p| p.tick != 0) {
            err(
                "tempo.first",
                "the first tempo point must be at tick 0".into(),
            );
        }
        if !self.tempo.windows(2).all(|w| w[0].tick < w[1].tick) {
            err(
                "tempo.order",
                "tempo points must have increasing ticks".into(),
            );
        }
        for p in &self.tempo {
            if !(p.bpm.is_finite() && (10.0..=1000.0).contains(&p.bpm)) {
                err("tempo.invalid", format!("bpm {} outside 10..=1000", p.bpm));
            }
        }
        if self.time_sig.first().is_none_or(|s| s.bar != 0) {
            err(
                "time_sig.first",
                "the first time signature must be at bar 0".into(),
            );
        }
        if !self.time_sig.windows(2).all(|w| w[0].bar < w[1].bar) {
            err(
                "time_sig.order",
                "time signatures must have increasing bars".into(),
            );
        }
        for s in &self.time_sig {
            if !(1..=32).contains(&s.num) || ![1, 2, 4, 8, 16, 32].contains(&s.den) {
                err(
                    "time_sig.invalid",
                    format!("{}/{} at bar {}", s.num, s.den, s.bar),
                );
            }
        }
        for s in &self.sections {
            if !(s.start >= 0 && s.start < s.end) {
                err(
                    "section.invalid",
                    format!("section `{}` needs 0 <= start < end", s.name),
                );
            }
        }
        let mut track_ids = BTreeSet::new();
        let mut note_ids = BTreeSet::new();
        for t in &self.tracks {
            if t.id.is_unassigned() || !track_ids.insert(t.id) {
                out.push(Diagnostic::error(
                    "track.id",
                    format!("{} is 0 or duplicated", t.id),
                ));
            }
            for n in &t.notes {
                if n.id.is_unassigned() || !note_ids.insert(n.id) {
                    out.push(
                        Diagnostic::error("note.id", format!("{} is 0 or duplicated", n.id))
                            .on_track(t.id)
                            .on_note(n.id),
                    );
                }
            }
            out.extend(track_issues(t));
            if !t
                .notes
                .windows(2)
                .all(|w| (w[0].start, w[0].id) < (w[1].start, w[1].id))
            {
                out.push(
                    Diagnostic::error("note.order", "notes must be sorted by (start, id)")
                        .on_track(t.id),
                );
            }
            out.extend(overlaps(t));
        }
        out
    }
}

/// Removes earlier elements with the same key as a later neighbor.
fn dedup_last<T, K: PartialEq>(v: &mut Vec<T>, key: impl Fn(&T) -> K) {
    let mut i = 0;
    while i + 1 < v.len() {
        if key(&v[i]) == key(&v[i + 1]) {
            v.remove(i);
        } else {
            i += 1;
        }
    }
}

/// What is wrong with a note's values, as messages.
pub(crate) fn note_issues(n: &Note) -> Vec<String> {
    let mut out = Vec::new();
    if n.start < 0 {
        out.push(format!("{}: start {} < 0", n.id, n.start));
    }
    if n.length <= 0 {
        out.push(format!("{}: length {} <= 0", n.id, n.length));
    }
    if n.key > 127 {
        out.push(format!("{}: key {} > 127", n.id, n.key));
    }
    let mut num = |id: &str, v: f64| {
        let d = note_param(id).expect("registered note param");
        if !d.accepts(v) {
            out.push(format!(
                "{}: {id} = {v} outside {}..={} {}",
                n.id, d.min, d.max, d.unit
            ));
        }
    };
    num("vel", n.vel);
    num("vol", n.vol);
    num("mod", n.mod_);
    if let Some(v) = n.timing.preutter_ms {
        num("timing.preutter_ms", v);
    }
    if let Some(v) = n.timing.overlap_ms {
        num("timing.overlap_ms", v);
    }
    num("timing.offset_ms", n.timing.offset_ms);
    let p = &n.pitch;
    num("portamento.length_ms", p.portamento.length_ms);
    num("portamento.start_ms", p.portamento.start_ms);
    num("portamento.overshoot_cents", p.portamento.overshoot_cents);
    num(
        "portamento.preparation_cents",
        p.portamento.preparation_cents,
    );
    for (name, bend) in [("scoop", p.scoop), ("fall", p.fall)] {
        if let Some(b) = bend {
            num(&format!("{name}.cents"), b.cents);
            num(&format!("{name}.length_ms"), b.length_ms);
        }
    }
    if let Some(v) = p.vibrato {
        num("vibrato.length", v.length);
        num("vibrato.period_ms", v.period_ms);
        num("vibrato.depth_cents", v.depth_cents);
        num("vibrato.fade_in", v.fade_in);
        num("vibrato.fade_out", v.fade_out);
        num("vibrato.phase", v.phase);
        num("vibrato.shift", v.shift);
        num("vibrato.drift", v.drift);
    }
    for q in &p.points {
        num("points.cents", q.cents);
    }
    if let Some(q) = p.points.iter().find(|q| !q.ms.is_finite()) {
        out.push(format!("{}: pitch point ms {} is not finite", n.id, q.ms));
    }
    out
}

/// Errors in a track's own values: notes, curves, effects and mix.
pub(crate) fn track_issues(t: &Track) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut push = |code: &str, msg: String| out.push(Diagnostic::error(code, msg).on_track(t.id));
    for n in &t.notes {
        for m in note_issues(n) {
            push("note.invalid", m);
        }
    }
    for (param, c) in &t.curves {
        let Some(d) = param.descriptor() else {
            push("param.unknown", format!("unknown curve `{param}`"));
            continue;
        };
        if let Some(&(tick, v)) = c.points.iter().find(|q| !d.accepts(q.1)) {
            push(
                "curve.value",
                format!(
                    "{param} value {v} at tick {tick} outside {}..={}",
                    d.min, d.max
                ),
            );
        }
    }
    for e in &t.effects {
        for m in e.issues() {
            push("effect.invalid", m);
        }
    }
    if !(-60.0..=12.0).contains(&t.mix.gain_db) || !(-1.0..=1.0).contains(&t.mix.pan) {
        push(
            "track.invalid",
            "gain_db must be in -60..=12 and pan in -1..=1".into(),
        );
    }
    out
}

/// An error for each note that overlaps an earlier one on its track.
pub(crate) fn overlaps(t: &Track) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut latest: Option<&Note> = None;
    for n in &t.notes {
        if let Some(prev) = latest
            && prev.end() > n.start
        {
            out.push(
                Diagnostic::error(
                    "note.overlap",
                    format!(
                        "{} ({}..{}) overlaps {} ({}..{}); tracks are monophonic",
                        n.id,
                        n.start,
                        n.end(),
                        prev.id,
                        prev.start,
                        prev.end()
                    ),
                )
                .on_track(t.id)
                .on_note(n.id),
            );
        }
        if latest.is_none_or(|l| n.end() > l.end()) {
            latest = Some(n);
        }
    }
    out
}
