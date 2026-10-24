//! RMVPE pitch estimation (Wei et al., "RMVPE: A Robust Model for Vocal
//! Pitch Estimation in Polyphonic Music", Interspeech 2023).
//!
//! The model (RVC's ONNX export) takes a log-mel spectrogram `[1, 128, T]`
//! and returns a 360-bin pitch salience per frame. Cantor supplies the
//! front end and decoding, matching RVC's `infer/rmvpe.py`: 16 kHz audio,
//! 1024-point Hann STFT with hop 160 (10 ms), 128 HTK mel bands 30–8000 Hz
//! with Slaney area normalization, natural log clamped at 1e-5; decoding
//! takes the salience-weighted mean of the cents of the 9 bins around the
//! peak, unvoiced when the peak salience is below 0.03.

use crate::onnx::{Model, tensor_f32};
use cantor_analysis::{F0Extractor, F0Track};
use realfft::RealFftPlanner;
use std::path::Path;

const SR: u32 = 16_000;
const N_FFT: usize = 1024;
const HOP: usize = 160;
const N_MELS: usize = 128;
const F_MIN: f64 = 30.0;
const F_MAX: f64 = 8_000.0;
const N_BINS: usize = 360;
const THRESHOLD: f32 = 0.03;

/// A loaded RMVPE model.
#[derive(Debug)]
pub struct Rmvpe {
    model: Model,
    input: &'static str,
    output: String,
    mel_basis: Vec<Vec<f64>>,
}

impl Rmvpe {
    /// Loads `rmvpe.onnx`.
    pub fn load(path: &Path) -> Result<Self, String> {
        let model = Model::load(path).map_err(|e| e.to_string())?;
        let input = model
            .input_names()
            .first()
            .cloned()
            .ok_or("RMVPE has no input")?;
        let output = model
            .output_names()
            .first()
            .cloned()
            .ok_or("RMVPE has no output")?;
        Ok(Self {
            // One allocation per loaded model; Model::run takes static names.
            input: Box::leak(input.into_boxed_str()),
            output,
            model,
            mel_basis: mel_filterbank(),
        })
    }

    fn log_mel(&self, x: &[f64]) -> (Vec<f32>, usize) {
        // center=True: reflect-pad by n_fft / 2.
        let pad = N_FFT / 2;
        let n = x.len();
        let get = |i: isize| -> f64 {
            if n == 0 {
                return 0.0;
            }
            let mut i = i;
            let last = n as isize - 1;
            while i < 0 || i > last {
                i = if i < 0 { -i } else { 2 * last - i };
            }
            x[i as usize]
        };
        let frames = 1 + n / HOP;
        let window: Vec<f64> = (0..N_FFT)
            .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / N_FFT as f64).cos())
            .collect();
        let fft = RealFftPlanner::<f64>::new().plan_fft_forward(N_FFT);
        let mut input = fft.make_input_vec();
        let mut spec = fft.make_output_vec();
        let mut mel = vec![0f32; N_MELS * frames];
        for f in 0..frames {
            let start = (f * HOP) as isize - pad as isize;
            for k in 0..N_FFT {
                input[k] = get(start + k as isize) * window[k];
            }
            fft.process(&mut input, &mut spec).expect("fixed-size FFT");
            let mag: Vec<f64> = spec.iter().map(|c| c.norm()).collect();
            for (m, filt) in self.mel_basis.iter().enumerate() {
                let e: f64 = filt.iter().zip(&mag).map(|(w, a)| w * a).sum();
                mel[m * frames + f] = e.max(1e-5).ln() as f32;
            }
        }
        (mel, frames)
    }
}

impl F0Extractor for Rmvpe {
    fn id(&self) -> &str {
        "rmvpe-rvc"
    }

    fn extract(&self, samples: &[f32], sample_rate: u32) -> Result<F0Track, String> {
        let x = resample(samples, sample_rate, SR);
        let (mel, frames) = self.log_mel(&x);
        // The network needs a frame count divisible by 32; pad with zeros.
        let padded = frames.div_ceil(32) * 32;
        let mut input = vec![0f32; N_MELS * padded];
        for m in 0..N_MELS {
            input[m * padded..m * padded + frames]
                .copy_from_slice(&mel[m * frames..(m + 1) * frames]);
        }
        let out = self
            .model
            .run(
                vec![(
                    self.input,
                    tensor_f32(&[1, N_MELS, padded], input).map_err(|e| e.to_string())?,
                )],
                0,
                0,
                &[&self.output],
            )
            .map_err(|e| e.to_string())?;
        let salience = &out[0];
        if salience.len() < frames * N_BINS {
            return Err(format!(
                "RMVPE returned {} values for {frames} frames",
                salience.len()
            ));
        }
        let mut hz = Vec::with_capacity(frames);
        let mut voiced = Vec::with_capacity(frames);
        for f in 0..frames {
            let row = &salience[f * N_BINS..(f + 1) * N_BINS];
            let (peak, &max) = row
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .expect("360 bins");
            if max < THRESHOLD {
                hz.push(0.0);
                voiced.push(false);
                continue;
            }
            let lo = peak.saturating_sub(4);
            let hi = (peak + 5).min(N_BINS);
            let (mut num, mut den) = (0.0f64, 0.0f64);
            for (i, &s) in row.iter().enumerate().take(hi).skip(lo) {
                num += f64::from(s) * cents_of_bin(i);
                den += f64::from(s);
            }
            let cents = num / den;
            hz.push((10.0 * 2f64.powf(cents / 1200.0)) as f32);
            voiced.push(true);
        }
        Ok(F0Track {
            hop_secs: HOP as f64 / f64::from(SR),
            t0: 0.0,
            hz,
            voiced,
        })
    }
}

/// RMVPE's bin → cents (relative to 10 Hz) mapping.
fn cents_of_bin(i: usize) -> f64 {
    20.0 * i as f64 + 1997.379_408_437_619_1
}

fn hz_to_mel_htk(f: f64) -> f64 {
    2595.0 * (1.0 + f / 700.0).log10()
}

fn mel_to_hz_htk(m: f64) -> f64 {
    700.0 * (10f64.powf(m / 2595.0) - 1.0)
}

/// librosa.filters.mel(sr=16000, n_fft=1024, n_mels=128, fmin=30,
/// fmax=8000, htk=True) with Slaney normalization.
fn mel_filterbank() -> Vec<Vec<f64>> {
    let n_freqs = N_FFT / 2 + 1;
    let fft_freqs: Vec<f64> = (0..n_freqs)
        .map(|k| k as f64 * f64::from(SR) / N_FFT as f64)
        .collect();
    let (mmin, mmax) = (hz_to_mel_htk(F_MIN), hz_to_mel_htk(F_MAX));
    let pts: Vec<f64> = (0..N_MELS + 2)
        .map(|i| mel_to_hz_htk(mmin + (mmax - mmin) * i as f64 / (N_MELS + 1) as f64))
        .collect();
    (0..N_MELS)
        .map(|m| {
            let (l, c, r) = (pts[m], pts[m + 1], pts[m + 2]);
            let enorm = 2.0 / (r - l);
            fft_freqs
                .iter()
                .map(|&f| {
                    let w = ((f - l) / (c - l)).min((r - f) / (r - c)).max(0.0);
                    w * enorm
                })
                .collect()
        })
        .collect()
}

/// Band-limited resampling with a Hann-windowed sinc (deterministic; good
/// enough for pitch analysis, not for playback).
pub fn resample(x: &[f32], from: u32, to: u32) -> Vec<f64> {
    if from == to {
        return x.iter().map(|&s| f64::from(s)).collect();
    }
    let ratio = f64::from(to) / f64::from(from);
    let cutoff = ratio.min(1.0) * 0.95;
    let half = 32.0 / cutoff;
    let n_out = (x.len() as f64 * ratio).floor() as usize;
    (0..n_out)
        .map(|j| {
            let t = j as f64 / ratio;
            let lo = (t - half).ceil().max(0.0) as usize;
            let hi = ((t + half).floor() as usize).min(x.len().saturating_sub(1));
            let mut acc = 0.0;
            for (i, &s) in x.iter().enumerate().take(hi + 1).skip(lo) {
                let d = i as f64 - t;
                let arg = std::f64::consts::PI * d * cutoff;
                let sinc = if d.abs() < 1e-12 {
                    1.0
                } else {
                    arg.sin() / arg
                };
                let w = 0.5 + 0.5 * (std::f64::consts::PI * d / half).cos();
                acc += f64::from(s) * cutoff * sinc * w;
            }
            acc
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filterbank_shape_and_normalization() {
        let fb = mel_filterbank();
        assert_eq!(fb.len(), 128);
        assert_eq!(fb[0].len(), 513);
        assert!(fb.iter().all(|row| row.iter().any(|w| *w > 0.0)));
    }

    #[test]
    fn resampling_keeps_a_tone() {
        let x: Vec<f32> = (0..44_100)
            .map(|i| (std::f32::consts::TAU * 440.0 * i as f32 / 44_100.0).sin())
            .collect();
        let y = resample(&x, 44_100, 16_000);
        assert_eq!(y.len(), 16_000);
        // Zero crossings ≈ 2 × 440 per second.
        let crossings = y.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        assert!((438..=442).contains(&crossings), "{crossings}");
    }
}
