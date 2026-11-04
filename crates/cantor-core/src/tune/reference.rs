//! `match_reference` and `humanize`: passes that write `pitd`
//! (docs/tuning.md §3 "Expression transfer" and §2 "Humanize").

use super::{Cx, TuneContext, hash_unit, round, simplify};
use crate::diag::{Diagnostic, Rejected};
use crate::ids::Tick;
use crate::model::Interp;
use crate::op::{NotePatch, Op, TimingPatch};
use crate::params::{ParamId, known};
use crate::pitch::{Noise, PitchOptions, STEP_SEC, render_pitch_with};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Parameters of `match_reference`. Writes (replaces) `pitd` over the range
/// so the final pitch follows the reference F0:
/// `pitd = strength · lowpass(fold(ref − base))`, where `base` is the pitch
/// from the notes without `pitd` and `fold` removes octave errors
/// (±1200 cents). Unvoiced gaps up to `max_gap_ms` are bridged linearly;
/// longer ones fade to 0 over 60 ms. Rerunning gives the same curve.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct MatchReferenceParams {
    /// How much of the difference to apply, 0..=1.5. Default 0.8.
    pub strength: f64,
    /// Low-pass window in ms (a centered moving average; 40 ms ≈ 25 Hz).
    /// Default 40.
    pub smooth_ms: f64,
    /// Clip `pitd` to ±this, in cents (≤ 1200). Default 300.
    pub max_cents: f64,
    /// Keep the reference's vibrato in `pitd` (cancelling the notes' own
    /// vibrato at strength 1). `false` smooths the reference's vibrato
    /// away (≈ 180 ms window) and keeps the notes' vibrato. Default true.
    pub keep_vibrato: bool,
    /// Largest unvoiced gap, in ms, bridged by interpolation. Default 250.
    pub max_gap_ms: f64,
    /// Curve simplification tolerance (RDP) in cents. Default 5.
    pub tolerance_cents: f64,
}

impl Default for MatchReferenceParams {
    fn default() -> Self {
        Self {
            strength: 0.8,
            smooth_ms: 40.0,
            max_cents: 300.0,
            keep_vibrato: true,
            max_gap_ms: 250.0,
            tolerance_cents: 5.0,
        }
    }
}

/// The tick span of the sung notes in scope, or `None`.
fn sung_span(cx: &Cx<'_>) -> Option<(Tick, Tick)> {
    let sung = cx.sung_in_range();
    let n = cx.notes();
    Some((
        n[*sung.first()?].start,
        sung.iter().map(|&i| n[i].end()).max()?,
    ))
}

pub(super) fn match_reference(
    cx: &mut Cx<'_>,
    p: &MatchReferenceParams,
    ctx: &TuneContext,
) -> Result<(), Rejected> {
    let Some(f0) = &ctx.reference_f0 else {
        return Err(Diagnostic::error(
            "tune.no_reference",
            "match_reference needs the reference F0; set a reference and analyze it",
        )
        .into());
    };
    if !f0.step_sec.is_finite() || f0.step_sec <= 0.0 {
        return Err(
            Diagnostic::error("tune.no_reference", "reference F0 has no frame step").into(),
        );
    }
    let Some((lo, hi)) = sung_span(cx) else {
        cx.summary = "no sung notes in range".into();
        return Ok(());
    };
    let (s0, s1) = (cx.tempo.tick_to_sec(lo), cx.tempo.tick_to_sec(hi));
    let opts = PitchOptions {
        pitd: false,
        vibrato: p.keep_vibrato,
        ..PitchOptions::default()
    };
    let base = render_pitch_with(cx.track, &cx.tempo, Some((s0, s1)), &opts);
    let mut diff: Vec<Option<f64>> = (0..base.len())
        .map(|i| {
            let r = f0.at(base.time(i)).filter(|_| base.voiced[i])?;
            let d = r - f64::from(base.cents[i]);
            Some(d - 1200.0 * (d / 1200.0).round())
        })
        .collect();
    let voiced = diff.iter().filter(|d| d.is_some()).count();
    if voiced == 0 {
        cx.note(Diagnostic::warning(
            "tune.no_overlap",
            "the reference is unvoiced wherever the notes sing",
        ));
        cx.summary = "no voiced reference under the notes".into();
        return Ok(());
    }
    let mut d = fill_gaps(&mut diff, p.max_gap_ms / 1000.0);
    let window = if p.keep_vibrato {
        p.smooth_ms
    } else {
        p.smooth_ms.max(180.0)
    };
    let passes = if p.keep_vibrato { 1 } else { 2 };
    for _ in 0..passes {
        d = box_filter(&d, (window / 1000.0 / STEP_SEC).round() as usize);
    }
    let k = p.strength.clamp(0.0, 1.5);
    let max = p.max_cents.clamp(0.0, 1200.0);
    let mut pts: Vec<(Tick, f64)> = Vec::with_capacity(d.len());
    for (i, v) in d.iter().enumerate() {
        let t = cx.tempo.sec_to_tick(base.time(i)).clamp(lo, hi);
        let v = round((k * v).clamp(-max, max), 10.0);
        match pts.last_mut() {
            Some(last) if last.0 == t => *last = (t, v),
            _ => pts.push((t, v)),
        }
    }
    let pts = simplify(&pts, p.tolerance_cents.max(0.0));
    cx.summary = format!(
        "pitd over {lo}..{hi}: {} points from {voiced} voiced frames, strength {k}",
        pts.len()
    );
    cx.push(Op::SetCurve {
        track: cx.track.id,
        param: ParamId::new(known::PITD),
        range: (lo, hi),
        points: pts,
        interp: Some(Interp::Linear),
    })
}

/// Fills `None` frames: linear across gaps up to `max_gap` seconds,
/// otherwise each side fades to 0 over 60 ms.
fn fill_gaps(v: &mut [Option<f64>], max_gap: f64) -> Vec<f64> {
    let fade = 0.06 / STEP_SEC;
    let mut out = vec![0.0; v.len()];
    let mut i = 0;
    while i < v.len() {
        if let Some(x) = v[i] {
            out[i] = x;
            i += 1;
            continue;
        }
        let a = i;
        while i < v.len() && v[i].is_none() {
            i += 1;
        }
        let left = a.checked_sub(1).and_then(|j| v[j]);
        let right = v.get(i).copied().flatten();
        let len = i - a;
        for (k, o) in out[a..i].iter_mut().enumerate() {
            let (dl, dr) = ((k + 1) as f64, (len - k) as f64);
            *o = match (left, right) {
                (Some(l), Some(r)) if len as f64 * STEP_SEC <= max_gap => {
                    l + (r - l) * dl / (len + 1) as f64
                }
                _ => {
                    left.unwrap_or(0.0) * (1.0 - dl / fade).max(0.0)
                        + right.unwrap_or(0.0) * (1.0 - dr / fade).max(0.0)
                }
            };
        }
    }
    out
}

/// Centered moving average over `w` frames (edges average what exists).
fn box_filter(v: &[f64], w: usize) -> Vec<f64> {
    if w <= 1 || v.is_empty() {
        return v.to_vec();
    }
    let mut sum = vec![0.0; v.len() + 1];
    for (i, x) in v.iter().enumerate() {
        sum[i + 1] = sum[i] + x;
    }
    let h = w / 2;
    (0..v.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(h), (i + w - h).min(v.len()));
            (sum[b] - sum[a]) / (b - a) as f64
        })
        .collect()
}

/// Parameters of `humanize`. Adds slow, seeded pitch drift to `pitd`
/// (on top of what is there, so running it twice doubles the drift: undo
/// first to re-roll) and, when `ms > 0`, sets each sung note's
/// `timing.offset_ms` to a seeded jitter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct HumanizeParams {
    /// Drift depth in cents (typical peak), 0..=100. Default 8.
    pub cents: f64,
    /// Timing jitter in ms (±), 0..=100; 0 leaves timing alone. Default 0.
    pub ms: f64,
    /// Seed; the same seed gives the same result. Default 0.
    pub seed: u64,
}

impl Default for HumanizeParams {
    fn default() -> Self {
        Self {
            cents: 8.0,
            ms: 0.0,
            seed: 0,
        }
    }
}

/// Drift sample spacing in seconds.
const DRIFT_STEP: f64 = 0.04;

pub(super) fn humanize(cx: &mut Cx<'_>, p: &HumanizeParams) -> Result<(), Rejected> {
    let Some((lo, hi)) = sung_span(cx) else {
        cx.summary = "no sung notes in range".into();
        return Ok(());
    };
    let seed = crate::mix64(p.seed ^ crate::mix64(cx.track.id.0));
    let cents = p.cents.clamp(0.0, 100.0);
    let mut parts = Vec::new();
    if cents > 0.0 {
        let noise = Noise::new(seed, [0.23, 0.41, 0.67]);
        let fine = Noise::new(crate::mix64(seed), [1.1, 1.7, 2.3]);
        let old = cx.track.curves.get(known::PITD);
        let (s0, s1) = (cx.tempo.tick_to_sec(lo), cx.tempo.tick_to_sec(hi));
        let mut ticks: Vec<Tick> = (0..=((s1 - s0) / DRIFT_STEP).ceil() as usize)
            .map(|i| {
                cx.tempo
                    .sec_to_tick(s0 + i as f64 * DRIFT_STEP)
                    .clamp(lo, hi)
            })
            .collect();
        if let Some(c) = old {
            ticks.extend(
                c.points
                    .iter()
                    .map(|q| q.0)
                    .filter(|t| (lo..=hi).contains(t)),
            );
        }
        ticks.sort_unstable();
        ticks.dedup();
        let pts: Vec<(Tick, f64)> = ticks
            .into_iter()
            .map(|t| {
                let s = cx.tempo.tick_to_sec(t);
                let drift = cents * (1.4 * noise.value(s) + 0.3 * fine.value(s));
                let v = old.and_then(|c| c.value_at(t)).unwrap_or(0.0) + drift;
                (t, round(v.clamp(-1200.0, 1200.0), 10.0))
            })
            .collect();
        let pts = simplify(&pts, 0.5);
        parts.push(format!("±{cents} cents drift ({} points)", pts.len()));
        cx.push(Op::SetCurve {
            track: cx.track.id,
            param: ParamId::new(known::PITD),
            range: (lo, hi),
            points: pts,
            interp: None,
        })?;
    }
    let ms = p.ms.clamp(0.0, 100.0);
    if ms > 0.0 {
        let notes = cx.notes();
        let ops: Vec<Op> = cx
            .sung_in_range()
            .into_iter()
            .map(|i| Op::UpdateNotes {
                track: cx.track.id,
                ids: vec![notes[i].id],
                patch: NotePatch {
                    timing: Some(TimingPatch {
                        offset_ms: Some(round(ms * hash_unit(seed, notes[i].id.0), 10.0)),
                        ..TimingPatch::default()
                    }),
                    ..NotePatch::default()
                },
            })
            .collect();
        parts.push(format!("±{ms} ms timing on {} notes", ops.len()));
        for op in ops {
            cx.push(op)?;
        }
    }
    cx.summary = parts.join(", ");
    Ok(())
}
