//! The per-track effects chain (docs/spec/40-algorithms.md §10):
//! EQ → compressor → de-esser → reverb, in the order the track lists them.
//!
//! Every effect is deterministic and processes a stereo pair in place.
//! Filter state is kept in f64 so long renders do not drift.

use cantor_core::{EffectKind, EffectSlot};
use std::f64::consts::PI;

/// Longest reverb tail appended to a track.
pub const MAX_TAIL_SECS: f64 = 10.0;

/// A stereo signal as two equal-length channels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stereo {
    /// Left channel.
    pub l: Vec<f32>,
    /// Right channel.
    pub r: Vec<f32>,
}

impl Stereo {
    /// Both channels equal to `mono`.
    pub fn from_mono(mono: &[f32]) -> Self {
        Self {
            l: mono.to_vec(),
            r: mono.to_vec(),
        }
    }

    /// Samples per channel.
    pub fn len(&self) -> usize {
        self.l.len()
    }

    /// Whether the signal is empty.
    pub fn is_empty(&self) -> bool {
        self.l.is_empty()
    }

    /// Extends both channels with silence to `len` samples.
    pub fn pad_to(&mut self, len: usize) {
        if len > self.l.len() {
            self.l.resize(len, 0.0);
            self.r.resize(len, 0.0);
        }
    }
}

/// Seconds of output an effect may produce after its input ends.
pub fn tail_secs(slot: &EffectSlot) -> f64 {
    if !slot.enabled || slot.kind != EffectKind::Reverb {
        return 0.0;
    }
    let v = |id| slot.value(id).expect("registered reverb parameter");
    if v("mix") == 0.0 {
        return 0.0;
    }
    let feedback = comb_feedback(v("room_size"));
    // RT60 of the longest comb: the time for its loop gain to reach -60 dB.
    let longest = COMB_TUNING[COMB_TUNING.len() - 1] as f64 + STEREO_SPREAD as f64;
    let rt60 = 3.0 * longest / (REF_RATE * -feedback.log10());
    (rt60 + v("pre_delay_ms") / 1000.0).min(MAX_TAIL_SECS)
}

/// Runs a track's chain over `signal`, first padding it with each enabled
/// effect's tail. Disabled slots are bypassed exactly.
pub fn process_chain(effects: &[EffectSlot], sample_rate: u32, signal: &mut Stereo) {
    let tail: f64 = effects
        .iter()
        .map(tail_secs)
        .sum::<f64>()
        .min(MAX_TAIL_SECS);
    if tail > 0.0 && !signal.is_empty() {
        let extra = (tail * f64::from(sample_rate)).ceil() as usize;
        signal.pad_to(signal.len() + extra);
    }
    for slot in effects.iter().filter(|s| s.enabled) {
        process(slot, sample_rate, signal);
    }
}

/// Runs one effect over `signal` without padding.
pub fn process(slot: &EffectSlot, sample_rate: u32, signal: &mut Stereo) {
    let v = |id: &str| slot.value(id).expect("registered effect parameter");
    let sr = f64::from(sample_rate);
    match slot.kind {
        EffectKind::Eq => eq(&v, sr, signal),
        EffectKind::Compressor => compressor(&v, sr, signal),
        EffectKind::DeEsser => de_esser(&v, sr, signal),
        EffectKind::Reverb => reverb(&v, sr, signal),
    }
}

// ---------------------------------------------------------------------------
// Biquads (RBJ Audio EQ Cookbook), transposed direct form II.

#[derive(Clone, Copy, Debug)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

enum Shape {
    LowPass,
    HighPass,
    LowShelf(f64),
    Peak(f64),
    HighShelf(f64),
}

impl Biquad {
    fn new(shape: Shape, freq: f64, q: f64, sr: f64) -> Self {
        let w0 = 2.0 * PI * freq.min(0.45 * sr) / sr;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let (b0, b1, b2, a0, a1, a2) = match shape {
            Shape::LowPass => (
                (1.0 - cos) / 2.0,
                1.0 - cos,
                (1.0 - cos) / 2.0,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
            Shape::HighPass => (
                (1.0 + cos) / 2.0,
                -(1.0 + cos),
                (1.0 + cos) / 2.0,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
            Shape::Peak(db) => {
                let a = 10f64.powf(db / 40.0);
                (
                    1.0 + alpha * a,
                    -2.0 * cos,
                    1.0 - alpha * a,
                    1.0 + alpha / a,
                    -2.0 * cos,
                    1.0 - alpha / a,
                )
            }
            Shape::LowShelf(db) | Shape::HighShelf(db) => {
                let a = 10f64.powf(db / 40.0);
                let k = 2.0 * a.sqrt() * alpha;
                // High shelf = low shelf with cos negated in the right places.
                let s = if matches!(shape, Shape::LowShelf(_)) {
                    1.0
                } else {
                    -1.0
                };
                (
                    a * ((a + 1.0) - s * (a - 1.0) * cos + k),
                    s * 2.0 * a * ((a - 1.0) - s * (a + 1.0) * cos),
                    a * ((a + 1.0) - s * (a - 1.0) * cos - k),
                    (a + 1.0) + s * (a - 1.0) * cos + k,
                    -s * 2.0 * ((a - 1.0) + s * (a + 1.0) * cos),
                    (a + 1.0) + s * (a - 1.0) * cos - k,
                )
            }
        };
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    #[inline]
    fn tick(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    fn run(&mut self, buf: &mut [f32]) {
        for s in buf {
            *s = self.tick(f64::from(*s)) as f32;
        }
    }
}

const BUTTERWORTH_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;
const SHELF_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;

fn eq(v: &dyn Fn(&str) -> f64, sr: f64, s: &mut Stereo) {
    let mut bands = Vec::new();
    if v("high_pass_hz") > 0.0 {
        bands.push(Biquad::new(
            Shape::HighPass,
            v("high_pass_hz"),
            BUTTERWORTH_Q,
            sr,
        ));
    }
    let gains = [
        (Shape::LowShelf(v("low_gain_db")), v("low_freq_hz"), SHELF_Q),
        (Shape::Peak(v("mid_gain_db")), v("mid_freq_hz"), v("mid_q")),
        (
            Shape::Peak(v("presence_gain_db")),
            v("presence_freq_hz"),
            v("presence_q"),
        ),
        (
            Shape::HighShelf(v("high_gain_db")),
            v("high_freq_hz"),
            SHELF_Q,
        ),
    ];
    for (shape, freq, q) in gains {
        let db = match shape {
            Shape::LowShelf(g) | Shape::Peak(g) | Shape::HighShelf(g) => g,
            Shape::LowPass | Shape::HighPass => unreachable!(),
        };
        if db != 0.0 {
            bands.push(Biquad::new(shape, freq, q, sr));
        }
    }
    for band in &bands {
        band.clone().run(&mut s.l);
        band.clone().run(&mut s.r);
    }
}

// ---------------------------------------------------------------------------
// Dynamics.

/// One-pole smoothing coefficient for a time constant.
fn coeff(ms: f64, sr: f64) -> f64 {
    (-1.0 / (ms.max(0.01) / 1000.0 * sr)).exp()
}

fn to_db(x: f64) -> f64 {
    20.0 * x.max(1e-12).log10()
}

/// Static gain curve: dB of reduction (≥ 0) for an input level in dB.
fn reduction_db(level: f64, threshold: f64, ratio: f64, knee: f64) -> f64 {
    let over = level - threshold;
    let slope = 1.0 - 1.0 / ratio;
    if knee > 0.0 && over.abs() <= knee / 2.0 {
        slope * (over + knee / 2.0).powi(2) / (2.0 * knee)
    } else if over > 0.0 {
        slope * over
    } else {
        0.0
    }
}

fn compressor(v: &dyn Fn(&str) -> f64, sr: f64, s: &mut Stereo) {
    let (threshold, ratio, knee) = (v("threshold_db"), v("ratio"), v("knee_db"));
    let (att, rel) = (coeff(v("attack_ms"), sr), coeff(v("release_ms"), sr));
    let makeup = 10f64.powf(v("makeup_db") / 20.0);
    // Smooth the gain reduction in dB: attack while it grows, release while
    // it shrinks. Stereo-linked so the image does not shift.
    let mut gr = 0.0f64;
    for i in 0..s.len() {
        let peak = f64::from(s.l[i].abs().max(s.r[i].abs()));
        let target = reduction_db(to_db(peak), threshold, ratio, knee);
        let c = if target > gr { att } else { rel };
        gr = target + c * (gr - target);
        let g = 10f64.powf(-gr / 20.0) * makeup;
        s.l[i] = (f64::from(s.l[i]) * g) as f32;
        s.r[i] = (f64::from(s.r[i]) * g) as f32;
    }
}

/// Compression ratio applied to the sibilance band above its threshold.
const DE_ESS_RATIO: f64 = 4.0;

/// A 4th-order Linkwitz–Riley crossover: two cascaded Butterworth
/// sections per band. The bands are in phase and sum to an all-pass, so
/// turning one band down changes only its magnitude.
#[derive(Clone, Copy)]
struct Crossover {
    lp: [Biquad; 2],
    hp: [Biquad; 2],
}

impl Crossover {
    fn new(freq: f64, sr: f64) -> Self {
        let lp = Biquad::new(Shape::LowPass, freq, BUTTERWORTH_Q, sr);
        let hp = Biquad::new(Shape::HighPass, freq, BUTTERWORTH_Q, sr);
        Self {
            lp: [lp, lp],
            hp: [hp, hp],
        }
    }

    /// `(low, high)` bands of one sample.
    fn split(&mut self, x: f64) -> (f64, f64) {
        let low = self.lp[0].tick(x);
        let high = self.hp[0].tick(x);
        (self.lp[1].tick(low), self.hp[1].tick(high))
    }
}

fn de_esser(v: &dyn Fn(&str) -> f64, sr: f64, s: &mut Stereo) {
    let (threshold, range) = (v("threshold_db"), v("range_db"));
    let mut xo_l = Crossover::new(v("freq_hz"), sr);
    let mut xo_r = xo_l;
    let (att, rel) = (coeff(1.0, sr), coeff(60.0, sr));
    let mut env = 0.0f64;
    for i in 0..s.len() {
        let (ll, hl) = xo_l.split(f64::from(s.l[i]));
        let (lr, hr) = xo_r.split(f64::from(s.r[i]));
        let level = hl.abs().max(hr.abs());
        let c = if level > env { att } else { rel };
        env = level + c * (env - level);
        let red = reduction_db(to_db(env), threshold, DE_ESS_RATIO, 0.0).min(range);
        let g = 10f64.powf(-red / 20.0);
        s.l[i] = (ll + g * hl) as f32;
        s.r[i] = (lr + g * hr) as f32;
    }
}

// ---------------------------------------------------------------------------
// Reverb: Schroeder–Moorer room (Freeverb tuning), eight damped feedback
// combs in parallel and four all-passes in series per channel.

/// Sample rate the delay tunings are specified at.
const REF_RATE: f64 = 44_100.0;
const COMB_TUNING: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNING: [usize; 4] = [556, 441, 341, 225];
const STEREO_SPREAD: usize = 23;
const INPUT_GAIN: f64 = 0.015;

fn comb_feedback(room_size: f64) -> f64 {
    0.7 + 0.28 * room_size
}

struct Comb {
    buf: Vec<f64>,
    pos: usize,
    store: f64,
}

impl Comb {
    fn tick(&mut self, x: f64, feedback: f64, damp: f64) -> f64 {
        let y = self.buf[self.pos];
        self.store = y * (1.0 - damp) + self.store * damp;
        self.buf[self.pos] = x + self.store * feedback;
        self.pos = (self.pos + 1) % self.buf.len();
        y
    }
}

struct Allpass {
    buf: Vec<f64>,
    pos: usize,
}

impl Allpass {
    fn tick(&mut self, x: f64) -> f64 {
        let b = self.buf[self.pos];
        self.buf[self.pos] = x + b * 0.5;
        self.pos = (self.pos + 1) % self.buf.len();
        b - x
    }
}

struct Channel {
    combs: Vec<Comb>,
    allpasses: Vec<Allpass>,
}

impl Channel {
    fn new(sr: f64, spread: usize) -> Self {
        let scale = |n: usize| (((n + spread) as f64) * sr / REF_RATE).round().max(1.0) as usize;
        Self {
            combs: COMB_TUNING
                .iter()
                .map(|&n| Comb {
                    buf: vec![0.0; scale(n)],
                    pos: 0,
                    store: 0.0,
                })
                .collect(),
            allpasses: ALLPASS_TUNING
                .iter()
                .map(|&n| Allpass {
                    buf: vec![0.0; scale(n)],
                    pos: 0,
                })
                .collect(),
        }
    }

    fn tick(&mut self, x: f64, feedback: f64, damp: f64) -> f64 {
        let mut y: f64 = self
            .combs
            .iter_mut()
            .map(|c| c.tick(x, feedback, damp))
            .sum();
        for a in &mut self.allpasses {
            y = a.tick(y);
        }
        y
    }
}

fn reverb(v: &dyn Fn(&str) -> f64, sr: f64, s: &mut Stereo) {
    let mix = v("mix");
    if mix == 0.0 {
        return;
    }
    let feedback = comb_feedback(v("room_size"));
    let damp = 0.4 * v("damping");
    let width = v("width");
    // Constant-power dry/wet balance.
    let (dry, wet) = ((1.0 - mix).sqrt(), mix.sqrt());
    let (wet1, wet2) = (wet * (0.5 + width / 2.0), wet * (0.5 - width / 2.0));
    let pre = (v("pre_delay_ms") / 1000.0 * sr).round() as usize;
    let mut left = Channel::new(sr, 0);
    let mut right = Channel::new(sr, STEREO_SPREAD);
    let mut pre_buf = vec![0.0f64; pre.max(1)];
    let mut pre_pos = 0;
    for i in 0..s.len() {
        let (xl, xr) = (f64::from(s.l[i]), f64::from(s.r[i]));
        let input = (xl + xr) * INPUT_GAIN;
        let delayed = if pre == 0 {
            input
        } else {
            let d = pre_buf[pre_pos];
            pre_buf[pre_pos] = input;
            pre_pos = (pre_pos + 1) % pre;
            d
        };
        let (yl, yr) = (
            left.tick(delayed, feedback, damp),
            right.tick(delayed, feedback, damp),
        );
        s.l[i] = (xl * dry + yl * wet1 + yr * wet2) as f32;
        s.r[i] = (xr * dry + yr * wet1 + yl * wet2) as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 44_100;

    fn sine(freq: f64, amp: f64, secs: f64) -> Vec<f32> {
        let n = (secs * f64::from(SR)) as usize;
        (0..n)
            .map(|i| (amp * (2.0 * PI * freq * i as f64 / f64::from(SR)).sin()) as f32)
            .collect()
    }

    /// RMS in dB over the second half (after filters and envelopes settle).
    fn rms_db(x: &[f32]) -> f64 {
        let tail = &x[x.len() / 2..];
        let p = tail.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / tail.len() as f64;
        10.0 * p.log10()
    }

    fn slot(kind: EffectKind, params: &[(&str, f64)]) -> EffectSlot {
        let mut s = EffectSlot::new(kind);
        for &(k, v) in params {
            s.params.insert(k.into(), v);
        }
        assert!(s.issues().is_empty(), "{:?}", s.issues());
        s
    }

    fn run(slot: &EffectSlot, x: &[f32]) -> Stereo {
        let mut s = Stereo::from_mono(x);
        process(slot, SR, &mut s);
        s
    }

    #[test]
    fn eq_boosts_cuts_and_high_passes() {
        let x = sine(1000.0, 0.25, 1.0);
        let base = rms_db(&x);
        let boosted = run(&slot(EffectKind::Eq, &[("mid_gain_db", 6.0)]), &x);
        assert!((rms_db(&boosted.l) - base - 6.0).abs() < 0.1);
        let low = sine(100.0, 0.25, 1.0);
        let cut = run(&slot(EffectKind::Eq, &[("high_pass_hz", 400.0)]), &low);
        // Two octaves below a 12 dB/oct cutoff: about -24 dB.
        assert!(rms_db(&cut.l) - rms_db(&low) < -20.0);
        let shelf = run(
            &slot(EffectKind::Eq, &[("high_gain_db", -12.0)]),
            &sine(16000.0, 0.25, 1.0),
        );
        assert!((rms_db(&shelf.l) - base + 12.0).abs() < 1.0);
        // All gains at 0 dB leave the signal bit-identical.
        assert_eq!(run(&EffectSlot::new(EffectKind::Eq), &x).l, x);
    }

    #[test]
    fn compressor_follows_its_static_curve() {
        // A -6 dBFS sine, 12 dB over a -18 dB threshold at 4:1 → 3 dB over.
        let x = sine(440.0, 0.5, 2.0);
        let c = slot(
            EffectKind::Compressor,
            &[
                ("threshold_db", -18.0),
                ("ratio", 4.0),
                ("knee_db", 0.0),
                ("release_ms", 500.0),
            ],
        );
        let y = run(&c, &x);
        let reduction = rms_db(&x) - rms_db(&y.l);
        assert!((reduction - 9.0).abs() < 0.5, "reduction {reduction:.2} dB");
        // Below the threshold nothing changes.
        let quiet = sine(440.0, 0.01, 1.0);
        assert!((rms_db(&run(&c, &quiet).l) - rms_db(&quiet)).abs() < 0.01);
    }

    #[test]
    fn de_esser_reduces_only_sibilance() {
        // A tone 1.3 octaves above the crossover lies almost wholly in the
        // sibilance band, so it is reduced by nearly the full range.
        let d = slot(
            EffectKind::DeEsser,
            &[
                ("freq_hz", 4000.0),
                ("threshold_db", -30.0),
                ("range_db", 8.0),
            ],
        );
        let ess = sine(10000.0, 0.3, 1.0);
        let red = rms_db(&ess) - rms_db(&run(&d, &ess).l);
        assert!((red - 8.0).abs() < 1.0, "sibilance reduced by {red:.2} dB");
        let vowel = sine(300.0, 0.3, 1.0);
        assert!((rms_db(&run(&d, &vowel).l) - rms_db(&vowel)).abs() < 0.1);
    }

    #[test]
    fn reverb_has_a_decaying_tail_and_is_deterministic() {
        let mut impulse = vec![0.0f32; 100];
        impulse[0] = 1.0;
        let r = slot(EffectKind::Reverb, &[("mix", 0.5), ("room_size", 0.5)]);
        let chain = [r.clone()];
        let mut a = Stereo::from_mono(&impulse);
        process_chain(&chain, SR, &mut a);
        let tail = tail_secs(&r);
        assert!(tail > 0.5 && tail < MAX_TAIL_SECS, "tail {tail}");
        assert_eq!(a.len(), 100 + (tail * f64::from(SR)).ceil() as usize);
        let energy = |x: &[f32]| x.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>();
        let sec = SR as usize;
        let (early, late) = (energy(&a.l[sec / 10..sec / 2]), energy(&a.l[sec..]));
        assert!(early > 0.0 && late < early, "early {early} late {late}");
        assert_ne!(a.l, a.r, "the tail is stereo");
        let mut b = Stereo::from_mono(&impulse);
        process_chain(&chain, SR, &mut b);
        assert_eq!(a, b);
    }

    #[test]
    fn disabled_and_dry_effects_are_exact_bypasses() {
        let x = sine(440.0, 0.5, 0.5);
        let mut off = slot(EffectKind::Compressor, &[("threshold_db", -40.0)]);
        off.enabled = false;
        let dry = slot(EffectKind::Reverb, &[("mix", 0.0)]);
        let mut s = Stereo::from_mono(&x);
        process_chain(&[off, dry], SR, &mut s);
        assert_eq!(s.l, x);
        assert_eq!(s.r, x);
    }
}
