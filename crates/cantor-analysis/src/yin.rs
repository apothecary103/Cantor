//! YIN F0 extraction with pYIN-style candidate tracking.
//!
//! Per frame: the YIN difference function (computed with FFTs), its
//! cumulative-mean-normalized form d′(τ), and every local dip of d′ in the
//! configured pitch range as a candidate (τ refined by parabolic
//! interpolation). A Viterbi pass then picks one candidate or "unvoiced"
//! per frame, trading periodicity against pitch continuity, which removes
//! most octave errors. Afterwards: short voiced blips are dropped, residual
//! octave jumps are corrected against a rolling median, and voiced runs are
//! median-filtered. Everything is deterministic.

use crate::{F0Extractor, F0Track, cents_to_hz, hz_to_cents};
use realfft::RealFftPlanner;
use realfft::num_complex::Complex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// YIN settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct YinConfig {
    /// Lowest F0 searched, Hz.
    pub fmin: f64,
    /// Highest F0 searched, Hz.
    pub fmax: f64,
    /// Seconds between frames.
    pub hop_secs: f64,
    /// YIN absolute threshold on d′: the first dip below it is the
    /// preferred period (guards against sub-octave errors).
    pub threshold: f64,
    /// Frames whose best dip of d′ is above this are unvoiced (with some
    /// hysteresis from the tracker).
    pub voicing_threshold: f64,
    /// Frames quieter than this (dBFS RMS) are unvoiced. Frames more than
    /// 55 dB below the loudest frame are unvoiced too.
    pub silence_db: f64,
    /// Width of the median filter applied within voiced runs, in frames
    /// (odd; 1 disables it).
    pub median_frames: usize,
    /// Voiced runs shorter than this many seconds become unvoiced.
    pub min_voiced_secs: f64,
}

impl Default for YinConfig {
    fn default() -> Self {
        Self {
            fmin: 60.0,
            fmax: 1100.0,
            hop_secs: 0.010,
            threshold: 0.15,
            voicing_threshold: 0.35,
            silence_db: -60.0,
            median_frames: 5,
            min_voiced_secs: 0.02,
        }
    }
}

/// The YIN extractor. [`Yin::for_samples`] (5 ms hop) suits voicebank
/// samples, [`Yin::for_reference`] (10 ms hop, the default) reference
/// vocals and renders.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Yin {
    /// Settings.
    pub config: YinConfig,
}

/// Dips of d′ kept per frame.
const MAX_CANDIDATES: usize = 8;
/// Dips with d′ above this are not candidates.
const CANDIDATE_CAP: f64 = 0.7;
/// Extra cost per octave below the YIN "first dip" choice.
const SUBOCTAVE_COST: f64 = 0.2;
/// Cost of a pitch jump of one octave beyond the free slope.
const JUMP_COST_PER_OCTAVE: f64 = 0.4;
/// Pitch slope (cents per second) followed for free: fast vibrato and
/// glides stay cheap.
const FREE_SLOPE: f64 = 4000.0;
/// Cost of switching between voiced and unvoiced.
const VOICING_SWITCH_COST: f64 = 0.15;

#[derive(Clone, Copy, Debug)]
struct Candidate {
    cents: f64,
    cost: f64,
}

impl Yin {
    /// An extractor with these settings.
    pub fn new(config: YinConfig) -> Self {
        Self { config }
    }

    /// 5 ms hop, for short voicebank samples.
    pub fn for_samples() -> Self {
        Self::new(YinConfig {
            hop_secs: 0.005,
            ..YinConfig::default()
        })
    }

    /// 10 ms hop, for reference vocals and rendered audio.
    pub fn for_reference() -> Self {
        Self::new(YinConfig::default())
    }

    /// F0 of mono `samples` at `sample_rate`. Frame `i` is centered at
    /// `i · hop`. `hz` is filled across unvoiced frames by interpolating
    /// between voiced neighbours (held at the ends; 0 if nothing is
    /// voiced), like a `.frq` file; `voiced` tells which frames are real.
    pub fn track(&self, samples: &[f32], sample_rate: u32) -> F0Track {
        let cfg = &self.config;
        let sr = f64::from(sample_rate.max(1));
        let hop = ((cfg.hop_secs * sr).round() as usize).max(1);
        let hop_secs = hop as f64 / sr;
        if samples.is_empty() {
            return F0Track {
                hop_secs,
                t0: 0.0,
                hz: Vec::new(),
                voiced: Vec::new(),
            };
        }
        let fmax = cfg.fmax.clamp(1.0, sr / 4.0);
        let fmin = cfg.fmin.clamp(1.0, fmax / 2.0);
        let tau_min = ((sr / fmax).floor() as usize).max(2);
        let tau_max = (sr / fmin).ceil() as usize;
        let w = tau_max.max((0.025 * sr).round() as usize);
        let len = w + tau_max + 2;
        let n_fft = len.next_power_of_two();
        let n_frames = samples.len() / hop + 1;

        let fft = RealFftPlanner::<f64>::new().plan_fft_forward(n_fft);
        let ifft = RealFftPlanner::<f64>::new().plan_fft_inverse(n_fft);
        let mut a_in = fft.make_input_vec();
        let mut b_in = fft.make_input_vec();
        let mut a_spec = fft.make_output_vec();
        let mut b_spec = fft.make_output_vec();
        let mut corr = ifft.make_output_vec();
        let mut scratch = fft.make_scratch_vec();
        let mut iscratch = ifft.make_scratch_vec();
        let mut cum = vec![0.0f64; len + 1];
        let mut d = vec![0.0f64; tau_max + 2];
        let mut dn = vec![1.0f64; tau_max + 2];

        let mut rms_db = Vec::with_capacity(n_frames);
        let mut cands: Vec<Vec<Candidate>> = Vec::with_capacity(n_frames);
        for i in 0..n_frames {
            // d(τ) compares x[0..w] with x[τ..τ+w], so its center is at
            // (w + τ)/2. Centering on (w + τ_min)/2 keeps the timing error
            // under half a period (centering on len/2 put frames up to
            // (τ_max − τ)/2 samples early: ~7 ms for a 440 Hz voice).
            let start = (i * hop) as isize - ((w + tau_min) / 2) as isize;
            b_in.fill(0.0);
            for (k, slot) in b_in.iter_mut().take(len).enumerate() {
                let idx = start + k as isize;
                if idx >= 0 && (idx as usize) < samples.len() {
                    *slot = f64::from(samples[idx as usize]);
                }
            }
            for k in 0..len {
                cum[k + 1] = cum[k] + b_in[k] * b_in[k];
            }
            let ms = cum[len] / len as f64;
            rms_db.push(if ms > 0.0 { 10.0 * ms.log10() } else { -200.0 });
            if ms <= 1e-20 {
                cands.push(Vec::new());
                continue;
            }
            a_in.fill(0.0);
            a_in[..w].copy_from_slice(&b_in[..w]);
            // Both transforms succeed: buffers come from the plans.
            let ok = fft
                .process_with_scratch(&mut a_in, &mut a_spec, &mut scratch)
                .is_ok()
                && fft
                    .process_with_scratch(&mut b_in, &mut b_spec, &mut scratch)
                    .is_ok();
            if !ok {
                cands.push(Vec::new());
                continue;
            }
            for (a, b) in a_spec.iter_mut().zip(&b_spec) {
                *a = a.conj() * b;
            }
            // The inverse needs purely real DC and Nyquist bins.
            a_spec[0] = Complex::new(a_spec[0].re, 0.0);
            if let Some(last) = a_spec.last_mut() {
                *last = Complex::new(last.re, 0.0);
            }
            if ifft
                .process_with_scratch(&mut a_spec, &mut corr, &mut iscratch)
                .is_err()
            {
                cands.push(Vec::new());
                continue;
            }
            let e0 = cum[w];
            let norm = 1.0 / n_fft as f64;
            for tau in 0..=tau_max + 1 {
                let et = cum[tau + w] - cum[tau];
                d[tau] = (e0 + et - 2.0 * corr[tau] * norm).max(0.0);
            }
            let mut sum = 0.0;
            dn[0] = 1.0;
            for tau in 1..=tau_max + 1 {
                sum += d[tau];
                dn[tau] = if sum > 0.0 {
                    d[tau] * tau as f64 / sum
                } else {
                    1.0
                };
            }
            cands.push(candidates(&d, &dn, tau_min, tau_max, sr, cfg.threshold));
        }

        let loudest = rms_db.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let floor = cfg.silence_db.max(loudest - 55.0);
        for (c, &db) in cands.iter_mut().zip(&rms_db) {
            if db < floor {
                c.clear();
            }
        }

        let path = viterbi(&cands, cfg.voicing_threshold, hop_secs);
        let mut cents: Vec<Option<f64>> = path
            .iter()
            .zip(&cands)
            .map(|(p, c)| p.map(|k| c[k].cents))
            .collect();
        let min_run = (cfg.min_voiced_secs / hop_secs).ceil() as usize;
        drop_short_runs(&mut cents, min_run);
        fix_octaves(
            &mut cents,
            &cands,
            ((0.1 / hop_secs).round() as usize).max(2),
        );
        median_runs(&mut cents, cfg.median_frames);

        let voiced: Vec<bool> = cents.iter().map(Option::is_some).collect();
        let filled = fill_gaps(&cents);
        F0Track {
            hop_secs,
            t0: 0.0,
            hz: filled
                .iter()
                .map(|c| c.map_or(0.0, |c| cents_to_hz(c) as f32))
                .collect(),
            voiced,
        }
    }
}

impl F0Extractor for Yin {
    fn id(&self) -> &str {
        "yin:1"
    }

    fn extract(&self, samples: &[f32], sample_rate: u32) -> Result<F0Track, String> {
        if sample_rate == 0 {
            return Err("sample rate is 0".into());
        }
        Ok(self.track(samples, sample_rate))
    }
}

/// Local dips of d′ in `[tau_min, tau_max]` as candidates, best first.
fn candidates(
    d: &[f64],
    dn: &[f64],
    tau_min: usize,
    tau_max: usize,
    sr: f64,
    threshold: f64,
) -> Vec<Candidate> {
    let mut dips: Vec<(f64, f64)> = Vec::new(); // (tau, d′)
    for tau in tau_min.max(1)..=tau_max {
        let v = dn[tau];
        if v < CANDIDATE_CAP && v <= dn[tau - 1] && v < dn[tau + 1] {
            // Refine the period on the raw difference function, which is
            // symmetric around the true period; the d′ value by parabola.
            let off = parabola(d[tau - 1], d[tau], d[tau + 1]).0;
            let val = parabola(dn[tau - 1], dn[tau], dn[tau + 1]).1;
            dips.push((tau as f64 + off, val.clamp(0.0, 1.0)));
        }
    }
    if dips.is_empty() {
        return Vec::new();
    }
    let first = dips
        .iter()
        .find(|(_, v)| *v < threshold)
        .or_else(|| dips.iter().min_by(|a, b| a.1.total_cmp(&b.1)))
        .map_or(1.0, |x| x.0);
    let mut out: Vec<Candidate> = dips
        .iter()
        .map(|&(tau, v)| Candidate {
            cents: hz_to_cents(sr / tau),
            cost: v + SUBOCTAVE_COST * (tau / first).log2().max(0.0),
        })
        .collect();
    out.sort_by(|a, b| a.cost.total_cmp(&b.cost));
    out.truncate(MAX_CANDIDATES);
    out
}

/// Vertex of the parabola through (−1, a), (0, b), (1, c): (offset, value),
/// offset clamped to ±1.
fn parabola(a: f64, b: f64, c: f64) -> (f64, f64) {
    let den = a - 2.0 * b + c;
    if den.abs() < 1e-18 {
        return (0.0, b);
    }
    let off = (0.5 * (a - c) / den).clamp(-1.0, 1.0);
    (off, b - 0.25 * (a - c) * off)
}

/// Best path through candidates (`Some(k)`) and "unvoiced" (`None`).
fn viterbi(cands: &[Vec<Candidate>], unvoiced_cost: f64, hop_secs: f64) -> Vec<Option<usize>> {
    let free = FREE_SLOPE * hop_secs;
    let jump =
        |a: f64, b: f64| JUMP_COST_PER_OCTAVE * (((a - b).abs() - free).max(0.0) / 1200.0).min(3.0);
    // State 0 is unvoiced, k + 1 is candidate k.
    let mut back: Vec<Vec<usize>> = Vec::with_capacity(cands.len());
    let mut prev: Vec<f64> = Vec::new();
    let mut prev_c: &[Candidate] = &[];
    for (i, c) in cands.iter().enumerate() {
        let mut cur = Vec::with_capacity(c.len() + 1);
        let mut bp = Vec::with_capacity(c.len() + 1);
        let u_emit = if c.is_empty() { 0.0 } else { unvoiced_cost };
        for s in 0..=c.len() {
            let emit = if s == 0 { u_emit } else { c[s - 1].cost };
            if i == 0 {
                cur.push(emit);
                bp.push(0);
                continue;
            }
            let mut best = (f64::INFINITY, 0);
            for (p, &pc) in prev.iter().enumerate() {
                let t = match (p, s) {
                    (0, 0) => 0.0,
                    (0, _) | (_, 0) => VOICING_SWITCH_COST,
                    _ => jump(prev_c[p - 1].cents, c[s - 1].cents),
                };
                if pc + t < best.0 {
                    best = (pc + t, p);
                }
            }
            cur.push(best.0 + emit);
            bp.push(best.1);
        }
        back.push(bp);
        prev = cur;
        prev_c = c;
    }
    let mut out = vec![None; cands.len()];
    let Some(mut s) = prev
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.total_cmp(b.1))
        .map(|x| x.0)
    else {
        return out;
    };
    for i in (0..cands.len()).rev() {
        out[i] = (s > 0).then(|| s - 1);
        s = back[i][s];
    }
    out
}

/// Voiced runs as half-open frame ranges.
pub(crate) fn runs<T>(x: &[Option<T>]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < x.len() {
        if x[i].is_some() {
            let a = i;
            while i < x.len() && x[i].is_some() {
                i += 1;
            }
            out.push((a, i));
        } else {
            i += 1;
        }
    }
    out
}

fn drop_short_runs(cents: &mut [Option<f64>], min_len: usize) {
    for (a, b) in runs(cents) {
        if b - a < min_len {
            cents[a..b].fill(None);
        }
    }
}

/// Median of a slice (sorts it). Empty → `None`.
pub(crate) fn median(v: &mut [f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    })
}

/// Frames more than 900 cents from the rolling median of their run take
/// the candidate nearest that median, or are shifted by whole octaves.
fn fix_octaves(cents: &mut [Option<f64>], cands: &[Vec<Candidate>], half: usize) {
    for (a, b) in runs(cents) {
        let orig: Vec<f64> = cents[a..b].iter().map(|c| c.unwrap_or(0.0)).collect();
        for i in a..b {
            let lo = i.saturating_sub(half).max(a);
            let hi = (i + half + 1).min(b);
            let mut win: Vec<f64> = orig[lo - a..hi - a].to_vec();
            let Some(med) = median(&mut win) else {
                continue;
            };
            let c = orig[i - a];
            if (c - med).abs() <= 900.0 {
                continue;
            }
            let near = cands[i]
                .iter()
                .map(|k| k.cents)
                .min_by(|x, y| (x - med).abs().total_cmp(&(y - med).abs()))
                .filter(|x| (x - med).abs() < 300.0);
            cents[i] = Some(near.unwrap_or_else(|| c + 1200.0 * ((med - c) / 1200.0).round()));
        }
    }
}

/// Median filter of width `width` inside each voiced run.
fn median_runs(cents: &mut [Option<f64>], width: usize) {
    let half = width / 2;
    if half == 0 {
        return;
    }
    for (a, b) in runs(cents) {
        let orig: Vec<f64> = cents[a..b].iter().map(|c| c.unwrap_or(0.0)).collect();
        for (i, c) in cents.iter_mut().enumerate().take(b).skip(a) {
            let lo = i.saturating_sub(half).max(a);
            let hi = (i + half + 1).min(b);
            let mut win = orig[lo - a..hi - a].to_vec();
            *c = median(&mut win);
        }
    }
}

/// Interpolates across unvoiced frames; holds at the ends.
fn fill_gaps(cents: &[Option<f64>]) -> Vec<Option<f64>> {
    let known: Vec<usize> = (0..cents.len()).filter(|&i| cents[i].is_some()).collect();
    if known.is_empty() {
        return vec![None; cents.len()];
    }
    let mut out = Vec::with_capacity(cents.len());
    let mut k = 0;
    for i in 0..cents.len() {
        while k + 1 < known.len() && known[k + 1] <= i {
            k += 1;
        }
        let (p, n) = (known[k], known.get(k + 1).copied());
        let v = if i <= p {
            cents[p]
        } else if let Some(n) = n {
            let (cp, cn) = (cents[p].unwrap_or(0.0), cents[n].unwrap_or(0.0));
            Some(cp + (cn - cp) * (i - p) as f64 / (n - p) as f64)
        } else {
            cents[p]
        };
        out.push(v);
    }
    out
}
