//! Artifact detectors: clipping, clicks where audio was joined, and
//! silence inside sung notes (usually a missing or broken sample).

use crate::rms_db;
use cantor_core::TrackId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Kinds of artifacts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    /// Samples at or beyond full scale.
    Clipping,
    /// A click (waveform jump) where note or phrase audio was joined.
    Discontinuity,
    /// Silence where a sung note should sound.
    UnexpectedSilence,
}

/// A detected artifact.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Artifact {
    /// Kind.
    pub kind: ArtifactKind,
    /// The track, or `None` for the mix.
    pub track: Option<TrackId>,
    /// Time in seconds.
    pub at_sec: f64,
    /// Duration in seconds.
    pub duration_sec: f64,
    /// Kind-specific magnitude: clipped samples, click strength (× the
    /// local curvature), or silence length in seconds.
    pub magnitude: f64,
    /// Human-readable explanation.
    pub detail: String,
}

/// Samples at or above this magnitude count as clipped.
pub const CLIP_LEVEL: f32 = 0.999;

/// Number of clipped samples (any channel) in interleaved audio.
pub fn clipped_samples(samples: &[f32]) -> u64 {
    samples.iter().filter(|s| s.abs() >= CLIP_LEVEL).count() as u64
}

/// Clipped stretches of interleaved audio, at most `limit` reported.
pub fn clipping(samples: &[f32], channels: usize, sr: u32, limit: usize) -> Vec<Artifact> {
    let ch = channels.max(1);
    let frames = samples.len() / ch;
    let clipped = |k: usize| {
        samples[k * ch..(k + 1) * ch]
            .iter()
            .any(|s| s.abs() >= CLIP_LEVEL)
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < frames && out.len() < limit {
        if clipped(i) {
            let start = i;
            // Runs separated by < 10 ms are one event.
            let mut last = i;
            while i < frames && (clipped(i) || i - last < (sr as usize / 100).max(1)) {
                if clipped(i) {
                    last = i;
                }
                i += 1;
            }
            let n = (start..=last).filter(|&k| clipped(k)).count();
            out.push(Artifact {
                kind: ArtifactKind::Clipping,
                track: None,
                at_sec: start as f64 / f64::from(sr),
                duration_sec: (last + 1 - start) as f64 / f64::from(sr),
                magnitude: n as f64,
                detail: format!("{n} clipped samples; lower track or master gain"),
            });
        }
        i += 1;
    }
    out
}

/// Clicks at join times `joins` (seconds) in mono `samples`: a
/// second-difference spike within ±2 ms of the join far above the
/// signal's curvature in the surrounding ±20 ms, at an audible level.
pub fn discontinuities(samples: &[f32], sr: u32, joins: &[f64], track: TrackId) -> Vec<Artifact> {
    let srf = f64::from(sr);
    let near = ((0.002 * srf) as usize).max(1);
    let around = ((0.020 * srf) as usize).max(near + 2);
    let mut out = Vec::new();
    for &j in joins {
        if !(j.is_finite() && j >= 0.0) {
            continue;
        }
        let c = (j * srf) as usize;
        if c < around + 1 || c + around + 1 >= samples.len() {
            continue;
        }
        if rms_db(&samples[c - around..c + around]) < -60.0 {
            continue;
        }
        let d2 = |k: usize| f64::from((samples[k + 1] - 2.0 * samples[k] + samples[k - 1]).abs());
        let spike = (c - near..c + near).map(d2).fold(0.0, f64::max);
        let mut rest: Vec<f64> = (c - around..c - near)
            .chain(c + near..c + around)
            .map(d2)
            .collect();
        rest.sort_by(f64::total_cmp);
        let typical = rest[rest.len() * 99 / 100].max(1e-7);
        let ratio = spike / typical;
        if ratio > 6.0 && spike > 1e-3 {
            out.push(Artifact {
                kind: ArtifactKind::Discontinuity,
                track: Some(track),
                at_sec: j,
                duration_sec: 0.0,
                magnitude: ratio,
                detail: format!(
                    "click at a join: curvature {ratio:.1}× the surrounding 99th percentile"
                ),
            });
        }
    }
    out
}

/// Silent stretches (< −60 dBFS for ≥ 100 ms) inside `[start, end)` spans
/// (seconds) of mono `samples`, ignoring the spans' outer 80 ms.
pub fn silences(samples: &[f32], sr: u32, spans: &[(f64, f64)], track: TrackId) -> Vec<Artifact> {
    let srf = f64::from(sr);
    let hop = ((0.010 * srf) as usize).max(1);
    let mut out = Vec::new();
    for &(s, e) in spans {
        let edge = 0.08f64.min((e - s) / 4.0);
        let a = (((s + edge) * srf).max(0.0) as usize).min(samples.len());
        let b = (((e - edge) * srf).max(0.0) as usize).min(samples.len());
        let mut run: Option<usize> = None;
        let mut k = a;
        let flush = |from: usize, to: usize, out: &mut Vec<Artifact>| {
            let dur = (to - from) as f64 / srf;
            if dur >= 0.1 {
                out.push(Artifact {
                    kind: ArtifactKind::UnexpectedSilence,
                    track: Some(track),
                    at_sec: from as f64 / srf,
                    duration_sec: dur,
                    magnitude: dur,
                    detail: format!(
                        "{:.0} ms of silence inside a sung note (missing or empty sample?)",
                        dur * 1000.0
                    ),
                });
            }
        };
        while k + hop <= b {
            let quiet = rms_db(&samples[k..k + hop]) < -60.0;
            match (quiet, run) {
                (true, None) => run = Some(k),
                (false, Some(r)) => {
                    flush(r, k, &mut out);
                    run = None;
                }
                _ => {}
            }
            k += hop;
        }
        if let Some(r) = run {
            flush(r, k, &mut out);
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
        let a = clipping(&s, 2, 1000, 10);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].kind, ArtifactKind::Clipping);
        assert_eq!(clipped_samples(&s), 2);
    }

    #[test]
    fn finds_join_click_only_where_present() {
        let sr = 44_100u32;
        let mut s: Vec<f32> = (0..sr)
            .map(|i| 0.3 * (std::f32::consts::TAU * 220.0 * i as f32 / sr as f32).sin())
            .collect();
        // A phase jump at 0.5 s.
        for (i, x) in s.iter_mut().enumerate().skip(sr as usize / 2) {
            *x = 0.3 * (std::f32::consts::TAU * 220.0 * i as f32 / sr as f32 + 2.0).sin();
        }
        let a = discontinuities(&s, sr, &[0.25, 0.5], TrackId(1));
        assert_eq!(a.len(), 1, "{a:?}");
        assert!((a[0].at_sec - 0.5).abs() < 1e-9);
    }
}
