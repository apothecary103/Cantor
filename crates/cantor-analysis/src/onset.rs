//! Onset detection: log-spectral flux, peak-picked against a local
//! median, kept only where the energy actually rises (so vibrato and
//! formant motion inside a held note do not count as onsets).

use crate::{percentile, rms_db};
use realfft::RealFftPlanner;

/// Onset detection settings.
#[derive(Clone, Debug, PartialEq)]
pub struct OnsetConfig {
    /// Seconds between analysis frames.
    pub hop_secs: f64,
    /// Minimum rise in dB of the 20 ms energy envelope from the 60 ms
    /// before the onset to the 50 ms after it.
    pub min_rise_db: f64,
    /// Onsets closer than this keep only the stronger one.
    pub min_interval_secs: f64,
    /// Onsets whose following 50 ms stays below this (dBFS) are ignored.
    pub floor_db: f64,
}

impl Default for OnsetConfig {
    fn default() -> Self {
        Self {
            hop_secs: 0.010,
            min_rise_db: 4.0,
            min_interval_secs: 0.06,
            floor_db: -50.0,
        }
    }
}

/// Onset times in seconds (ascending) of mono `samples`.
pub fn onsets(samples: &[f32], sample_rate: u32) -> Vec<f64> {
    onsets_with(samples, sample_rate, &OnsetConfig::default())
}

/// [`onsets`] with explicit settings.
pub fn onsets_with(samples: &[f32], sample_rate: u32, cfg: &OnsetConfig) -> Vec<f64> {
    let sr = f64::from(sample_rate.max(1));
    let hop = ((cfg.hop_secs * sr).round() as usize).max(1);
    let hop_secs = hop as f64 / sr;
    let n_fft = ((0.023 * sr) as usize).next_power_of_two();
    let n_frames = samples.len() / hop + 1;
    if samples.len() < n_fft / 2 {
        return Vec::new();
    }
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(n_fft);
    let window: Vec<f64> = (0..n_fft)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n_fft as f64).cos())
        .collect();
    let max_bin = ((11_000.0 / sr * n_fft as f64) as usize).min(n_fft / 2);
    let mut input = fft.make_input_vec();
    let mut spec = fft.make_output_vec();
    let mut scratch = fft.make_scratch_vec();
    let mut prev = vec![0.0f64; max_bin];
    let mut flux = Vec::with_capacity(n_frames);
    let env_half = ((0.010 * sr) as usize).max(1);
    let mut env = Vec::with_capacity(n_frames);
    for i in 0..n_frames {
        let c = i * hop;
        let start = c as isize - (n_fft / 2) as isize;
        for (k, x) in input.iter_mut().enumerate() {
            let idx = start + k as isize;
            *x = if idx >= 0 && (idx as usize) < samples.len() {
                f64::from(samples[idx as usize]) * window[k]
            } else {
                0.0
            };
        }
        let mut f = 0.0;
        if fft
            .process_with_scratch(&mut input, &mut spec, &mut scratch)
            .is_ok()
        {
            for (k, p) in prev.iter_mut().enumerate() {
                let l = (1.0 + 100.0 * spec[k].norm()).ln();
                if i > 0 {
                    f += (l - *p).max(0.0);
                }
                *p = l;
            }
        }
        flux.push(f);
        let a = c.saturating_sub(env_half).min(samples.len());
        let b = (c + env_half).min(samples.len());
        env.push(rms_db(&samples[a..b]));
    }

    let mut sorted = flux.clone();
    let delta = 0.1 * percentile(&mut sorted, 95.0).unwrap_or(0.0);
    let span = |secs: f64| ((secs / hop_secs).round() as usize).max(1);
    let (med_half, peak_half) = (span(0.25), span(0.03));
    let (before, after) = (span(0.06), span(0.05));
    let mut found: Vec<(usize, f64)> = Vec::new();
    for i in 1..n_frames {
        let lo = i.saturating_sub(peak_half);
        let hi = (i + peak_half + 1).min(n_frames);
        if flux[lo..hi].iter().any(|&x| x > flux[i]) || flux[i] <= 0.0 {
            continue;
        }
        let mut local = flux[i.saturating_sub(med_half)..(i + med_half + 1).min(n_frames)].to_vec();
        let thr = 1.5 * percentile(&mut local, 50.0).unwrap_or(0.0) + delta;
        if flux[i] <= thr {
            continue;
        }
        let pre = env[i.saturating_sub(before)..i]
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min);
        let post = env[i..(i + after + 1).min(n_frames)]
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        if post < cfg.floor_db || post - pre.min(post) < cfg.min_rise_db {
            continue;
        }
        match found.last_mut() {
            Some(last) if (i - last.0) as f64 * hop_secs < cfg.min_interval_secs => {
                if flux[i] > last.1 {
                    *last = (i, flux[i]);
                }
            }
            _ => found.push((i, flux[i])),
        }
    }
    found
        .into_iter()
        .map(|(i, _)| ((i as f64 - 0.5) * hop_secs).max(0.0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_note_attacks_not_vibrato() {
        let sr = 44_100;
        let mut x = vec![0.0f32; sr * 3];
        // Two attacks at 0.5 s and 1.5 s; the second note has strong
        // vibrato for a second.
        let mut ph = 0.0f64;
        for (i, s) in x.iter_mut().enumerate() {
            let t = i as f64 / sr as f64;
            let (start, vib) = if t >= 1.5 { (1.5, 1.0) } else { (0.5, 0.0) };
            let f = 300.0 * (1.0 + 0.03 * vib * (std::f64::consts::TAU * 6.0 * t).sin());
            ph += std::f64::consts::TAU * f / sr as f64;
            if t < start || (t > 1.3 && t < 1.5) {
                continue;
            }
            let env = ((t - start) / 0.01).min(1.0);
            *s = (0.3 * env * (ph.sin() + 0.5 * (2.0 * ph).sin())) as f32;
        }
        let o = onsets(&x, sr as u32);
        assert_eq!(o.len(), 2, "{o:?}");
        assert!((o[0] - 0.5).abs() < 0.02, "{o:?}");
        assert!((o[1] - 1.5).abs() < 0.02, "{o:?}");
    }
}
