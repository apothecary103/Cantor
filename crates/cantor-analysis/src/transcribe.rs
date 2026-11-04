//! Reference F0 → notes, per docs/tuning.md §3:
//!
//! 1. Subtract the global tuning offset (circular mean of the cents
//!    residue over voiced frames).
//! 2. Voiced runs, bridging unvoiced gaps shorter than `gap_ms`.
//! 3. Within a run, a new note starts when the lightly smoothed pitch
//!    stays more than `split_cents` from the current note's median, on the
//!    same side, for `hold_ms`. Vibrato swings cross the threshold only
//!    briefly and alternate sides, so they don't split. The boundary is
//!    placed where the pitch crosses midway between the two notes.
//! 4. Energy onsets split repeated notes inside a run.
//! 5. Segments shorter than `min_note_ms` are ornaments and join a
//!    neighbour; adjacent segments that round to the same key merge
//!    unless an onset separates them.
//! 6. Note pitch = median of the middle 50% (in time) of its frames.
//! 7. Gaps shorter than `close_gaps_ms` are closed (the next syllable's
//!    consonant lives there), then starts/ends are quantized to `grid`;
//!    the start residue is kept as timing.

use crate::ReferenceF0;
use crate::yin::{median, runs};
use cantor_core::{Note, TempoMap, Tick};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Transcription settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct TranscribeOptions {
    /// Quantization grid in ticks (120 = 1/16 at 480 PPQ; 0 = no
    /// quantization beyond whole ticks).
    pub grid: Tick,
    /// Shorter segments are ornaments, not notes.
    pub min_note_ms: f64,
    /// Unvoiced gaps at least this long end a note.
    pub gap_ms: f64,
    /// A pitch move larger than this (cents) ...
    pub split_cents: f64,
    /// ... held this long starts a new note.
    pub hold_ms: f64,
    /// Split on energy onsets (repeated notes).
    pub use_onsets: bool,
    /// Estimate and remove a global tuning offset (e.g. A = 435 Hz).
    pub correct_tuning: bool,
    /// Gaps between notes shorter than this are closed by extending the
    /// earlier note.
    pub close_gaps_ms: f64,
    /// Lyric for every note.
    pub lyric: String,
}

impl Default for TranscribeOptions {
    fn default() -> Self {
        Self {
            grid: 120,
            min_note_ms: 80.0,
            gap_ms: 40.0,
            split_cents: 60.0,
            hold_ms: 70.0,
            use_onsets: true,
            correct_tuning: true,
            close_gaps_ms: 150.0,
            lyric: "a".into(),
        }
    }
}

/// A transcription with the measurements behind it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Transcription {
    /// The notes, in time order, non-overlapping, ids unassigned.
    pub notes: Vec<Note>,
    /// Global tuning offset that was removed, in cents (+ = sharp).
    pub tuning_offset_cents: f64,
    /// Per note: unquantized pitch in cents (offset removed).
    pub pitch_cents: Vec<f64>,
    /// Per note: sung start minus quantized start, in ms (+ = late).
    pub start_residue_ms: Vec<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Cut {
    /// Start of a voiced run.
    Run,
    /// Pitch move.
    Pitch,
    /// Energy onset.
    Onset,
}

#[derive(Clone, Copy, Debug)]
struct Seg {
    a: usize,
    b: usize,
    cut: Cut,
}

/// Notes transcribed from a reference (see the module docs).
pub fn transcribe(
    reference: &ReferenceF0,
    tempo: &TempoMap,
    opts: &TranscribeOptions,
) -> Vec<Note> {
    transcribe_detailed(reference, tempo, opts).notes
}

/// [`transcribe`] plus tuning offset, raw pitches and timing residues.
pub fn transcribe_detailed(
    reference: &ReferenceF0,
    tempo: &TempoMap,
    opts: &TranscribeOptions,
) -> Transcription {
    let step = reference.step_sec;
    let empty = Transcription {
        notes: Vec::new(),
        tuning_offset_cents: 0.0,
        pitch_cents: Vec::new(),
        start_residue_ms: Vec::new(),
    };
    if step <= 0.0 || reference.is_empty() {
        return empty;
    }
    let frames = |ms: f64| ((ms / 1000.0 / step).round() as usize).max(1);

    let offset = if opts.correct_tuning {
        tuning_offset(&reference.cents)
    } else {
        0.0
    };
    let c: Vec<Option<f64>> = reference
        .cents
        .iter()
        .map(|x| x.map(|v| f64::from(v) - offset))
        .collect();

    // Voiced runs, bridging short gaps.
    let gap = frames(opts.gap_ms);
    let mut bridged: Vec<(usize, usize)> = Vec::new();
    for (a, b) in runs(&c) {
        match bridged.last_mut() {
            Some(last) if a - last.1 < gap => last.1 = b,
            _ => bridged.push((a, b)),
        }
    }

    let smooth = smoothed(&c, frames(40.0));
    let mut segs: Vec<Seg> = Vec::new();
    for &(a, b) in &bridged {
        let first = segs.len();
        split_pitch(&c, &smooth, a, b, opts, frames, &mut segs);
        if opts.use_onsets {
            split_onsets(reference, &mut segs, first, opts, frames);
        }
        absorb_ornaments(&mut segs, first, frames(opts.min_note_ms));
    }
    // Same-key neighbours split only by pitch merge.
    let mut i = 1;
    while i < segs.len() {
        let (p, s) = (segs[i - 1], segs[i]);
        if s.cut == Cut::Pitch
            && p.b == s.a
            && key_of(note_pitch(&c, p)) == key_of(note_pitch(&c, s))
        {
            segs[i - 1].b = s.b;
            segs.remove(i);
        } else {
            i += 1;
        }
    }
    let min_frames = frames(opts.min_note_ms);
    segs.retain(|s| s.b - s.a >= min_frames);

    // Times, gap closing, quantization.
    let t = |f: usize| reference.start_sec + f as f64 * step;
    let mut spans: Vec<(f64, f64, f64)> = segs
        .iter()
        .filter_map(|&s| note_pitch(&c, s).map(|p| (t(s.a), t(s.b), p)))
        .collect();
    for k in 1..spans.len() {
        if spans[k].0 - spans[k - 1].1 < opts.close_gaps_ms / 1000.0 {
            spans[k - 1].1 = spans[k].0;
        }
    }
    let q = |sec: f64| {
        let tick = tempo.sec_to_tick_f64(sec);
        if opts.grid > 0 {
            ((tick / opts.grid as f64).round() as Tick) * opts.grid
        } else {
            tick.round() as Tick
        }
    };
    let unit = opts.grid.max(1);
    let mut out = empty;
    out.tuning_offset_cents = offset;
    for &(s, e, p) in &spans {
        let mut start = q(s).max(0);
        let mut end = q(e).max(start + unit);
        if let Some(prev) = out.notes.last_mut()
            && start < prev.end()
        {
            if start >= prev.start + unit {
                prev.length = start - prev.start;
            } else {
                start = prev.end();
                end = end.max(start + unit);
            }
        }
        end = end.max(start + unit);
        out.notes.push(Note::new(
            start,
            end - start,
            key_of(Some(p)).unwrap_or(60),
            opts.lyric.clone(),
        ));
        out.pitch_cents.push(p);
        out.start_residue_ms
            .push((s - tempo.tick_to_sec(start)) * 1000.0);
    }
    out
}

fn key_of(p: Option<f64>) -> Option<u8> {
    p.map(|p| (p / 100.0).round().clamp(0.0, 127.0) as u8)
}

/// Circular mean of the residue `cents mod 100`, in (−50, 50].
fn tuning_offset(cents: &[Option<f32>]) -> f64 {
    let (mut s, mut co, mut n) = (0.0, 0.0, 0usize);
    for c in cents.iter().flatten() {
        let ph = std::f64::consts::TAU * f64::from(*c) / 100.0;
        s += ph.sin();
        co += ph.cos();
        n += 1;
    }
    if n == 0 || (s * s + co * co).sqrt() < 1e-9 * n as f64 {
        return 0.0;
    }
    s.atan2(co) * 100.0 / std::f64::consts::TAU
}

/// Mean of voiced frames within ±`half` of each frame.
fn smoothed(c: &[Option<f64>], half: usize) -> Vec<Option<f64>> {
    let half = half / 2;
    (0..c.len())
        .map(|i| {
            c[i]?;
            let lo = i.saturating_sub(half);
            let hi = (i + half + 1).min(c.len());
            let v: Vec<f64> = c[lo..hi].iter().flatten().copied().collect();
            Some(v.iter().sum::<f64>() / v.len() as f64)
        })
        .collect()
}

/// Splits run `[a, b)` on held pitch moves.
fn split_pitch(
    c: &[Option<f64>],
    smooth: &[Option<f64>],
    a: usize,
    b: usize,
    opts: &TranscribeOptions,
    frames: impl Fn(f64) -> usize,
    out: &mut Vec<Seg>,
) {
    let hold = frames(opts.hold_ms);
    let settle = frames(50.0);
    let lookback = frames(60.0);
    let mut seg_a = a;
    // Sorted pitches of the current segment after its first `settle`
    // frames (the scoop into it).
    let mut sorted: Vec<f64> = Vec::new();
    let mut pending: Option<(usize, f64)> = None;
    let mut i = a;
    while i < b {
        let (Some(ci), Some(si)) = (c[i], smooth[i]) else {
            i += 1;
            continue;
        };
        let center = if sorted.len() >= 3 {
            Some(sorted[sorted.len() / 2])
        } else {
            None
        };
        let dev = center.map(|m| si - m);
        match (dev, pending) {
            (Some(d), _) if d.abs() <= opts.split_cents => pending = None,
            (Some(d), Some((_, sign))) if d.signum() != sign => pending = Some((i, d.signum())),
            (Some(d), None) => pending = Some((i, d.signum())),
            _ => {}
        }
        if let (Some((p, _)), Some(m)) = (pending, center)
            && i - p + 1 >= hold
        {
            let mut new: Vec<f64> = c[p..=i].iter().flatten().copied().collect();
            let new_m = median(&mut new).unwrap_or(ci);
            let mid = 0.5 * (m + new_m);
            let up = new_m > m;
            let from = p.saturating_sub(lookback).max(seg_a + 1);
            let cut = (from..=i)
                .find(|&j| c[j].is_some_and(|v| if up { v > mid } else { v < mid }))
                .unwrap_or(p);
            out.push(Seg {
                a: seg_a,
                b: cut,
                cut: if seg_a == a { Cut::Run } else { Cut::Pitch },
            });
            seg_a = cut;
            sorted.clear();
            pending = None;
            i = cut;
            continue;
        }
        if i >= seg_a + settle || i + 1 == b {
            let pos = sorted.partition_point(|&x| x < ci);
            sorted.insert(pos, ci);
        }
        i += 1;
    }
    out.push(Seg {
        a: seg_a,
        b,
        cut: if seg_a == a { Cut::Run } else { Cut::Pitch },
    });
}

/// Splits segments from index `first` on energy onsets away from their
/// edges.
fn split_onsets(
    r: &ReferenceF0,
    segs: &mut Vec<Seg>,
    first: usize,
    opts: &TranscribeOptions,
    frames: impl Fn(f64) -> usize,
) {
    let min = frames(opts.min_note_ms.max(60.0));
    for &t in &r.onsets {
        let k = ((t - r.start_sec) / r.step_sec).round();
        if k < 0.0 {
            continue;
        }
        let k = k as usize;
        if let Some(idx) =
            (first..segs.len()).find(|&j| segs[j].a + min <= k && k + min <= segs[j].b)
        {
            let s = segs[idx];
            segs[idx].b = k;
            segs.insert(
                idx + 1,
                Seg {
                    a: k,
                    b: s.b,
                    cut: Cut::Onset,
                },
            );
        }
    }
}

/// Segments (from index `first`) shorter than `min` frames join the
/// previous segment of their run, or the next if they start it.
fn absorb_ornaments(segs: &mut Vec<Seg>, first: usize, min: usize) {
    let mut i = first;
    while i < segs.len() {
        if segs[i].b - segs[i].a >= min || segs.len() - first == 1 {
            i += 1;
            continue;
        }
        if i > first && segs[i].cut != Cut::Run {
            segs[i - 1].b = segs[i].b;
            segs.remove(i);
        } else if i + 1 < segs.len() && segs[i + 1].a == segs[i].b {
            segs[i + 1].a = segs[i].a;
            segs[i + 1].cut = segs[i].cut;
            segs.remove(i);
        } else {
            i += 1;
        }
    }
}

/// Median of the middle 50% (in time) of a segment's voiced frames.
fn note_pitch(c: &[Option<f64>], s: Seg) -> Option<f64> {
    let v: Vec<f64> = c[s.a..s.b].iter().flatten().copied().collect();
    let n = v.len();
    if n == 0 {
        return None;
    }
    let (lo, hi) = (n / 4, (3 * n).div_ceil(4).max(n / 4 + 1));
    let mut mid = v[lo..hi.min(n)].to_vec();
    median(&mut mid)
}
