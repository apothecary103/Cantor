//! Artifact detectors: clipping, discontinuities at phrase joins, crackle,
//! unexpected silence and formant jumps.

use crate::TrackInput;
use realfft::RealFftPlanner;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Kinds of artifacts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    /// Samples at or beyond full scale.
    Clipping,
    /// A jump in the waveform where phrase audio was joined.
    Discontinuity,
    /// Short bursts of high-frequency energy.
    Crackle,
    /// Silence where a sung note should sound.
    UnexpectedSilence,
    /// An abrupt change of spectral envelope within a sung note.
    FormantJump,
}

/// A detected artifact.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Artifact {
    /// Kind.
    pub kind: ArtifactKind,
    /// The track, or `None` for the mix.
    pub track: Option<cantor_core::TrackId>,
    /// Time in seconds.
    pub at_sec: f64,
    /// Duration in seconds.
    pub duration_sec: f64,
    /// Detector-specific magnitude (see `detail`).
    pub magnitude: f64,
    /// Human-readable explanation.
    pub detail: String,
}

/// Samples at or above this magnitude count as clipped.
pub const CLIP_LEVEL: f32 = 0.999;

/// Mix-level detectors (clipping).
pub fn detect_mix(stereo: &[f32], sr: u32) -> Vec<Artifact> {
    let mut out = Vec::new();
    let frames = stereo.len() / 2;
    let mut i = 0;
    while i < frames {
        let clipped =
            |k: usize| stereo[2 * k].abs() >= CLIP_LEVEL || stereo[2 * k + 1].abs() >= CLIP_LEVEL;
        if clipped(i) {
            let start = i;
            while i < frames && clipped(i) {
                i += 1;
            }
            out.push(Artifact {
                kind: ArtifactKind::Clipping,
                track: None,
                at_sec: start as f64 / f64::from(sr),
                duration_sec: (i - start) as f64 / f64::from(sr),
                magnitude: (i - start) as f64,
                detail: format!("{} clipped samples", i - start),
            });
        }
        i += 1;
    }
    out
}

/// Track-level detectors.
pub fn detect_track(t: &TrackInput<'_>) -> Vec<Artifact> {
    let mut out = Vec::new();
    out.extend(discontinuities(t));
    out.extend(silence(t));
    out.extend(spectral(t));
    out
}

/// RMS of a sample range.
fn rms(x: &[f32]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}

/// Sample-to-sample jumps at joins, relative to the local signal.
fn discontinuities(t: &TrackInput<'_>) -> Vec<Artifact> {
    let sr = f64::from(t.sample_rate);
    let w = (0.010 * sr) as usize;
    let mut out = Vec::new();
    for &j in &t.joins {
        let c = (j * sr) as usize;
        if c < 2 * w || c + 2 * w >= t.samples.len() {
            continue;
        }
        let local = &t.samples[c - 2 * w..c + 2 * w];
        let level = rms(local);
        if level < 1e-4 {
            continue;
        }
        let diffs: Vec<f64> = local
            .windows(2)
            .map(|p| f64::from((p[1] - p[0]).abs()))
            .collect();
        let mut sorted = diffs.clone();
        sorted.sort_by(f64::total_cmp);
        let typical = sorted[sorted.len() / 2].max(1e-6);
        let worst = sorted[sorted.len() - 1];
        if worst > 8.0 * typical && worst > 2.0 * level {
            out.push(Artifact {
                kind: ArtifactKind::Discontinuity,
                track: Some(t.track),
                at_sec: j,
                duration_sec: 0.0,
                magnitude: worst / typical,
                detail: format!(
                    "waveform jump {:.1}× the typical step at a phrase join",
                    worst / typical
                ),
            });
        }
    }
    out
}

/// Silent stretches (< −60 dBFS for ≥ 100 ms) inside sung notes' steady parts.
fn silence(t: &TrackInput<'_>) -> Vec<Artifact> {
    let sr = f64::from(t.sample_rate);
    let hop = (0.010 * sr) as usize;
    let threshold = 10f64.powf(-60.0 / 20.0);
    let mut out = Vec::new();
    for n in t.notes.iter().filter(|n| n.sung) {
        let edge = 0.08f64.min((n.end_sec - n.start_sec) / 4.0);
        let a = ((n.start_sec + edge) * sr) as usize;
        let b = (((n.end_sec - edge) * sr) as usize).min(t.samples.len());
        let mut run_start = None;
        let mut k = a;
        while k + hop <= b {
            let quiet = rms(&t.samples[k..k + hop]) < threshold;
            match (quiet, run_start) {
                (true, None) => run_start = Some(k),
                (false, Some(s)) => {
                    push_silence(&mut out, t, s, k, sr);
                    run_start = None;
                }
                _ => {}
            }
            k += hop;
        }
        if let Some(s) = run_start {
            push_silence(&mut out, t, s, k, sr);
        }
    }
    out
}

fn push_silence(out: &mut Vec<Artifact>, t: &TrackInput<'_>, a: usize, b: usize, sr: f64) {
    let dur = (b - a) as f64 / sr;
    if dur >= 0.1 {
        out.push(Artifact {
            kind: ArtifactKind::UnexpectedSilence,
            track: Some(t.track),
            at_sec: a as f64 / sr,
            duration_sec: dur,
            magnitude: dur,
            detail: format!("{:.0} ms of silence inside a sung note", dur * 1000.0),
        });
    }
}

/// Crackle (high-frequency energy spikes) and formant jumps (spectral
/// envelope changes within voiced note bodies), from 2048-point spectra
/// every 10 ms.
fn spectral(t: &TrackInput<'_>) -> Vec<Artifact> {
    let sr = f64::from(t.sample_rate);
    let n_fft = 2048;
    let hop = (0.010 * sr) as usize;
    if t.samples.len() < n_fft {
        return Vec::new();
    }
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(n_fft);
    let window: Vec<f64> = (0..n_fft)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n_fft as f64).cos())
        .collect();
    let bin_hz = sr / n_fft as f64;
    let hf_bin = (8000.0 / bin_hz) as usize;
    let mut input = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    let mut hf_ratio = Vec::new();
    let mut envelopes: Vec<Vec<f64>> = Vec::new();
    let mut times = Vec::new();
    let mut k = 0;
    while k + n_fft <= t.samples.len() {
        for i in 0..n_fft {
            input[i] = f64::from(t.samples[k + i]) * window[i];
        }
        if fft.process(&mut input, &mut spectrum).is_err() {
            return Vec::new();
        }
        let power: Vec<f64> = spectrum.iter().map(|c| c.norm_sqr()).collect();
        let total: f64 = power.iter().sum::<f64>() + 1e-12;
        hf_ratio.push(power[hf_bin..].iter().sum::<f64>() / total);
        // Coarse envelope: log energy in 24 bands up to 8 kHz.
        let bands = 24;
        let env: Vec<f64> = (0..bands)
            .map(|b| {
                let lo = 1 + b * hf_bin / bands;
                let hi = 1 + (b + 1) * hf_bin / bands;
                (power[lo..hi].iter().sum::<f64>() / total + 1e-9).log10()
            })
            .collect();
        envelopes.push(env);
        times.push((k + n_fft / 2) as f64 / sr);
        k += hop;
    }
    let mut out = Vec::new();
    let mut sorted = hf_ratio.clone();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[sorted.len() / 2];
    for i in 1..hf_ratio.len().saturating_sub(1) {
        let spike = hf_ratio[i] > 0.25 && hf_ratio[i] > 6.0 * median.max(1e-4);
        let isolated = hf_ratio[i - 1] < hf_ratio[i] / 2.0 && hf_ratio[i + 1] < hf_ratio[i] / 2.0;
        if spike && isolated {
            out.push(Artifact {
                kind: ArtifactKind::Crackle,
                track: Some(t.track),
                at_sec: times[i],
                duration_sec: 0.01,
                magnitude: hf_ratio[i],
                detail: format!(
                    "{:.0}% of energy above 8 kHz in one frame",
                    hf_ratio[i] * 100.0
                ),
            });
        }
    }
    let in_steady = |time: f64| {
        t.notes.iter().any(|n| {
            let edge = 0.08f64.min((n.end_sec - n.start_sec) / 4.0);
            n.sung && time > n.start_sec + edge && time < n.end_sec - edge
        })
    };
    for i in 1..envelopes.len() {
        if !in_steady(times[i]) || !in_steady(times[i - 1]) {
            continue;
        }
        let d: f64 = envelopes[i]
            .iter()
            .zip(&envelopes[i - 1])
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f64>()
            .sqrt();
        if d > 3.0 {
            out.push(Artifact {
                kind: ArtifactKind::FormantJump,
                track: Some(t.track),
                at_sec: times[i],
                duration_sec: 0.01,
                magnitude: d,
                detail: format!("spectral envelope changed by {d:.1} (log-band distance) in 10 ms"),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_clipping() {
        let mut s = vec![0.1f32; 2000];
        s[100] = 1.0;
        s[102] = 1.0;
        let a = detect_mix(&s, 1000);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].kind, ArtifactKind::Clipping);
    }
}
