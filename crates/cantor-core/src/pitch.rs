//! The final pitch curve: the single source of truth shared by the
//! synthesizer, the tuner and the API (docs/design.md, "Model").
//!
//! ```text
//! final(t) = key·100 + transition (portamento, overshoot, preparation)
//!          + scoop + fall + vibrato + note points + pitd(t)        [cents]
//! ```
//! sampled on a 5 ms grid anchored at 0 s. Rests and breaths are unvoiced.
//! Everything here is pure, deterministic math (vibrato drift is seeded by
//! the note id).
//!
//! - Owner: at time t the pitch belongs to the last sung note that started
//!   at or before t (or the first note, before the song starts); in a gap
//!   it holds the previous note's key.
//! - Transition: a note *connected* to the previous sung note (gap below
//!   [`GLIDE_GAP_SEC`], no rest in between) glides from the previous key.
//!   With `a = start + start_ms`, `L = length_ms`, the path runs through
//!   `(a − P, from)`, `(a, from − s·preparation)`, `(a + L, to + s·overshoot)`,
//!   `(a + L + S, to)` where `s` is the direction of the jump,
//!   `P = clamp(0.75·L, 30, 100) ms` (only with preparation) and
//!   `S = clamp(L, 80, 150) ms` (only with overshoot). The main segment
//!   uses the portamento shape; the others ease in and out. `length_ms = 0`
//!   or an unconnected note is a hard step at the note start.
//! - Scoop: `cents·(1 − ease(u))` over the first `length_ms` of the note.
//! - Fall: `cents·ease_in(u)` over the last `length_ms` (at most the note).
//! - Vibrato (OpenUtau semantics): covers the last `length`% of the note;
//!   fades in/out over `fade_in`/`fade_out`% of the vibrato;
//!   `depth·env·(sin(2π(x/period + phase)) + shift/100)`; `drift` wanders the
//!   rate by up to ±6% and the depth by up to ±20% with slow noise seeded
//!   by the note id.
//! - Points: `(ms, cents)` relative to the note start, eased between points
//!   and held before the first and after the last, within the note.

use crate::ids::Tick;
use crate::mix64;
use crate::model::{Curve, Note, Portamento, Shape, Track, Vibrato};
use crate::params::known::PITD;
use crate::time::TempoMap;
use std::f64::consts::{PI, TAU};

/// The grid step of rendered pitch curves, in seconds.
pub const STEP_SEC: f64 = 0.005;

/// Notes closer than this (end of one to start of the next) glide; further
/// apart they step.
pub const GLIDE_GAP_SEC: f64 = 0.05;

/// Rate wander at `drift = 1`, as a fraction of the rate.
const RATE_WANDER: f64 = 0.06;
/// Depth wander at `drift = 1`, as a fraction of the depth.
const DEPTH_WANDER: f64 = 0.2;

/// A pitch curve on a regular grid.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PitchCurve {
    /// Time of the first sample, in seconds (a multiple of `step_sec`).
    pub start_sec: f64,
    /// Sample spacing in seconds ([`STEP_SEC`]).
    pub step_sec: f64,
    /// Pitch in cents (MIDI note × 100). Unvoiced samples hold the nearest
    /// note's pitch, so the curve is defined everywhere.
    pub cents: Vec<f32>,
    /// Whether a note is sung at each sample (false in rests and breaths).
    pub voiced: Vec<bool>,
}

impl PitchCurve {
    /// Number of samples.
    pub fn len(&self) -> usize {
        self.cents.len()
    }

    /// Whether there are no samples.
    pub fn is_empty(&self) -> bool {
        self.cents.is_empty()
    }

    /// Time of sample `i` in seconds.
    pub fn time(&self, i: usize) -> f64 {
        self.start_sec + i as f64 * self.step_sec
    }

    /// Linearly interpolated pitch at `sec`, or `None` outside the curve.
    pub fn at(&self, sec: f64) -> Option<f64> {
        let x = (sec - self.start_sec) / self.step_sec;
        let last = self.cents.len().checked_sub(1)?;
        if !(0.0..=last as f64).contains(&x) {
            return None;
        }
        let i = (x.floor() as usize).min(last);
        let j = (i + 1).min(last);
        let u = x - i as f64;
        Some(f64::from(self.cents[i]) * (1.0 - u) + f64::from(self.cents[j]) * u)
    }
}

/// What to include when rendering.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PitchOptions {
    /// Largest gap between notes, in seconds, across which they glide.
    pub glide_gap_sec: f64,
    /// Add the track's `pitd` curve.
    pub pitd: bool,
    /// Add note vibratos.
    pub vibrato: bool,
}

impl Default for PitchOptions {
    fn default() -> Self {
        Self {
            glide_gap_sec: GLIDE_GAP_SEC,
            pitd: true,
            vibrato: true,
        }
    }
}

/// The final pitch of a track over `range_sec` (default: 0 s to the end of
/// the last note).
pub fn render_pitch(track: &Track, tempo: &TempoMap, range_sec: Option<(f64, f64)>) -> PitchCurve {
    render_pitch_with(track, tempo, range_sec, &PitchOptions::default())
}

/// The pitch from the notes alone, without the `pitd` curve.
pub fn base_pitch(track: &Track, tempo: &TempoMap, range_sec: Option<(f64, f64)>) -> PitchCurve {
    let opts = PitchOptions {
        pitd: false,
        ..PitchOptions::default()
    };
    render_pitch_with(track, tempo, range_sec, &opts)
}

/// A sung note with its times and, when it glides in, the previous pitch.
struct Sung<'a> {
    shape: NoteShape<'a>,
    from: Option<f64>,
}

/// The sung notes of a track, with their connection to the previous note.
fn sung_notes<'a>(track: &'a Track, tempo: &'a TempoMap, gap: f64) -> Vec<Sung<'a>> {
    let mut out = Vec::new();
    let mut prev: Option<(&Note, f64)> = None;
    for tn in track.timed_notes(tempo) {
        if !tn.note.is_rest_or_breath() {
            let from = prev
                .filter(|(p, end)| !p.is_rest_or_breath() && tn.start_sec - end < gap)
                .map(|(p, _)| key_cents(p));
            out.push(Sung {
                shape: NoteShape::new(tn.note, tn.start_sec, tn.end_sec),
                from,
            });
        }
        prev = Some((tn.note, tn.end_sec));
    }
    out
}

/// [`render_pitch`] with options.
pub fn render_pitch_with(
    track: &Track,
    tempo: &TempoMap,
    range_sec: Option<(f64, f64)>,
    opts: &PitchOptions,
) -> PitchCurve {
    let sung = sung_notes(track, tempo, opts.glide_gap_sec);
    let end = sung.last().map_or(0.0, |s| s.shape.end);
    let (r0, r1) = range_sec.unwrap_or((0.0, end));
    let i0 = (r0.max(0.0) / STEP_SEC).floor() as i64;
    let n = (((r1 - i0 as f64 * STEP_SEC) / STEP_SEC).ceil().max(1.0)) as usize;
    let t_at = |i: usize| (i0 + i as i64) as f64 * STEP_SEC;
    let mut out = PitchCurve {
        start_sec: t_at(0),
        step_sec: STEP_SEC,
        cents: vec![0.0; n],
        voiced: vec![false; n],
    };
    if sung.is_empty() {
        return out;
    }
    let mut cents = vec![0.0f64; n];
    // Owner pitch and voicing.
    let mut j = 0;
    for i in 0..n {
        let t = t_at(i);
        while j + 1 < sung.len() && sung[j + 1].shape.start <= t {
            j += 1;
        }
        let o = &sung[j].shape;
        cents[i] = key_cents(o.note);
        out.voiced[i] =
            t >= o.start && (t < o.end || sung.get(j + 1).is_some_and(|nx| nx.from.is_some()));
    }
    // Frame index of the first sample at or after `t`.
    let idx = |t: f64| ((t / STEP_SEC).ceil() as i64 - i0).clamp(0, n as i64) as usize;
    for s in &sung {
        let sh = &s.shape;
        for (i, c) in cents
            .iter_mut()
            .enumerate()
            .take(idx(sh.end))
            .skip(idx(sh.start))
        {
            *c += sh.offset(t_at(i), opts.vibrato);
        }
        if let Some(from) = s.from {
            let (a, b) = transition_span(sh.note, sh.start);
            for (i, c) in cents.iter_mut().enumerate().take(idx(b)).skip(idx(a)) {
                *c += transition_offset(from, sh.note, sh.start, t_at(i));
            }
        }
    }
    if opts.pitd
        && let Some(curve) = track.curves.get(PITD).filter(|c| !c.points.is_empty())
    {
        add_curve(&mut cents, curve, tempo, &t_at);
    }
    out.cents = cents.into_iter().map(|c| c as f32).collect();
    out
}

fn add_curve(cents: &mut [f64], curve: &Curve, tempo: &TempoMap, t_at: &impl Fn(usize) -> f64) {
    let mut last: Option<(Tick, f64)> = None;
    for (i, c) in cents.iter_mut().enumerate() {
        let tick = tempo.sec_to_tick(t_at(i));
        let v = match last {
            Some((t, v)) if t == tick => v,
            _ => curve.value_at(tick).unwrap_or(0.0),
        };
        last = Some((tick, v));
        *c += v;
    }
}

fn key_cents(n: &Note) -> f64 {
    f64::from(n.key) * 100.0
}

/// Easing of a glide, `u` in 0..=1.
pub fn ease(shape: Shape, u: f64) -> f64 {
    let u = u.clamp(0.0, 1.0);
    match shape {
        Shape::S => (1.0 - (PI * u).cos()) / 2.0,
        Shape::Linear => u,
        Shape::In => 1.0 - (PI / 2.0 * u).cos(),
        Shape::Out => (PI / 2.0 * u).sin(),
    }
}

fn lerp(a: f64, b: f64, u: f64) -> f64 {
    a + (b - a) * u
}

/// Where a note's transition (preparation, glide, overshoot settle) can
/// differ from a plain step, in seconds: `[start, end)`.
pub fn transition_span(note: &Note, start_sec: f64) -> (f64, f64) {
    let k = TransitionKnots::new(&note.pitch.portamento, start_sec);
    (k.a - k.prep, k.a + k.len + k.settle)
}

struct TransitionKnots {
    a: f64,
    len: f64,
    prep: f64,
    settle: f64,
}

impl TransitionKnots {
    fn new(p: &Portamento, start_sec: f64) -> Self {
        let len = (p.length_ms / 1000.0).max(0.0);
        let prep = if p.preparation_cents > 0.0 {
            (0.75 * len).clamp(0.03, 0.1)
        } else {
            0.0
        };
        let settle = if p.overshoot_cents > 0.0 {
            len.clamp(0.08, 0.15)
        } else {
            0.0
        };
        Self {
            a: start_sec + p.start_ms / 1000.0,
            len,
            prep,
            settle,
        }
    }
}

/// The glide into `note` from the pitch `from_cents`, relative to a hard
/// step at the note start (so 0 outside [`transition_span`]).
pub fn transition_offset(from_cents: f64, note: &Note, start_sec: f64, t: f64) -> f64 {
    let p = &note.pitch.portamento;
    let to = key_cents(note);
    let d = to - from_cents;
    if p.length_ms <= 0.0 || d == 0.0 {
        return 0.0;
    }
    let s = d.signum();
    let k = TransitionKnots::new(p, start_sec);
    let low = from_cents - s * p.preparation_cents;
    let high = to + s * p.overshoot_cents;
    let v = if t < k.a - k.prep {
        from_cents
    } else if t < k.a {
        lerp(from_cents, low, ease(Shape::S, (t - k.a + k.prep) / k.prep))
    } else if t < k.a + k.len {
        lerp(low, high, ease(p.shape, (t - k.a) / k.len))
    } else if t < k.a + k.len + k.settle {
        lerp(high, to, ease(Shape::S, (t - k.a - k.len) / k.settle))
    } else {
        to
    };
    v - if t < start_sec { from_cents } else { to }
}

/// A note's own pitch expression (scoop, fall, vibrato and points),
/// prepared for evaluation.
#[derive(Clone, Debug)]
pub struct NoteShape<'a> {
    /// The note.
    pub note: &'a Note,
    /// Start in seconds.
    pub start: f64,
    /// End in seconds.
    pub end: f64,
    /// Bend points as (seconds from the note start, cents), sorted.
    points: Vec<(f64, f64)>,
    rate_noise: Noise,
    depth_noise: Noise,
}

impl<'a> NoteShape<'a> {
    /// Prepares a note spanning `start..end` seconds.
    pub fn new(note: &'a Note, start: f64, end: f64) -> Self {
        let mut points: Vec<(f64, f64)> = note
            .pitch
            .points
            .iter()
            .map(|p| (p.ms / 1000.0, p.cents))
            .collect();
        points.sort_by(|a, b| a.0.total_cmp(&b.0));
        let seed = mix64(note.id.0);
        Self {
            note,
            start,
            end,
            points,
            rate_noise: Noise::new(seed, DRIFT_FREQS),
            depth_noise: Noise::new(mix64(seed), DRIFT_FREQS),
        }
    }

    /// Scoop + fall + vibrato (if `vibrato`) + points at `t`, in cents;
    /// 0 outside the note.
    pub fn offset(&self, t: f64, vibrato: bool) -> f64 {
        if t < self.start || t >= self.end {
            return 0.0;
        }
        let p = &self.note.pitch;
        let mut c = 0.0;
        if let Some(b) = p.scoop.filter(|b| b.length_ms > 0.0) {
            let u = (t - self.start) / (b.length_ms / 1000.0);
            if u < 1.0 {
                c += b.cents * (1.0 - ease(Shape::S, u));
            }
        }
        if let Some(b) = p.fall.filter(|b| b.length_ms > 0.0) {
            let len = (b.length_ms / 1000.0).min(self.end - self.start);
            let u = (t - (self.end - len)) / len;
            if u >= 0.0 {
                c += b.cents * ease(Shape::In, u);
            }
        }
        if vibrato {
            c += self.vibrato(t);
        }
        c + self.points_at(t - self.start)
    }

    /// The vibrato alone at `t`, in cents; 0 outside its window.
    pub fn vibrato(&self, t: f64) -> f64 {
        let Some(v) = &self.note.pitch.vibrato else {
            return 0.0;
        };
        let vlen = (self.end - self.start) * v.length.clamp(0.0, 100.0) / 100.0;
        let x = t - (self.end - vlen);
        if x < 0.0 || x >= vlen || v.depth_cents == 0.0 || v.period_ms <= 0.0 {
            return 0.0;
        }
        vibrato_value(v, x, vlen, &self.rate_noise, &self.depth_noise)
    }

    fn points_at(&self, x: f64) -> f64 {
        let pts = &self.points;
        let (Some(first), Some(last)) = (pts.first(), pts.last()) else {
            return 0.0;
        };
        if x <= first.0 {
            return first.1;
        }
        if x >= last.0 {
            return last.1;
        }
        let i = pts.partition_point(|p| p.0 <= x) - 1;
        let ((x0, y0), (x1, y1)) = (pts[i], pts[i + 1]);
        lerp(y0, y1, ease(Shape::S, (x - x0) / (x1 - x0)))
    }
}

/// Vibrato at `x` seconds into a window of `vlen` seconds.
fn vibrato_value(v: &Vibrato, x: f64, vlen: f64, rate: &Noise, depth: &Noise) -> f64 {
    let fi = vlen * v.fade_in / 100.0;
    let fo = vlen * v.fade_out / 100.0;
    let mut env: f64 = 1.0;
    if fi > 0.0 {
        env = env.min(x / fi);
    }
    if fo > 0.0 {
        env = env.min((vlen - x) / fo);
    }
    let hz = 1000.0 / v.period_ms;
    let cycles = hz * (x + RATE_WANDER * v.drift * rate.integral(x)) + v.phase;
    let d = v.depth_cents * (1.0 + DEPTH_WANDER * v.drift * depth.value(x));
    d * env.clamp(0.0, 1.0) * ((TAU * cycles).sin() + v.shift / 100.0)
}

/// Slow noise frequencies for vibrato drift, in Hz.
const DRIFT_FREQS: [f64; 3] = [0.31, 0.53, 0.97];

/// Smooth seeded noise in [-1, 1]: the mean of three sinusoids with random
/// phases, with a closed-form integral.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Noise {
    parts: [(f64, f64); 3],
}

impl Noise {
    /// Noise with the given frequencies (Hz) and seed.
    pub(crate) fn new(seed: u64, freqs: [f64; 3]) -> Self {
        let mut s = seed;
        let parts = freqs.map(|f| {
            s = mix64(s);
            (f, (s >> 11) as f64 / (1u64 << 53) as f64 * TAU)
        });
        Self { parts }
    }

    /// The noise at `t` seconds.
    pub(crate) fn value(&self, t: f64) -> f64 {
        self.parts
            .iter()
            .map(|&(f, phi)| (TAU * f * t + phi).sin())
            .sum::<f64>()
            / 3.0
    }

    /// ∫₀ᵗ noise.
    fn integral(&self, t: f64) -> f64 {
        self.parts
            .iter()
            .map(|&(f, phi)| {
                let w = TAU * f;
                (phi.cos() - (w * t + phi).cos()) / w
            })
            .sum::<f64>()
            / 3.0
    }
}
