//! Pitch accuracy: extracted F0 against the score's intended pitch.

use crate::{F0Track, NoteTarget, hz_to_cents, percentile};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Frames within this distance of a note boundary are transitions and are
/// not scored (portamento and consonants live there).
pub const EDGE_SECS: f64 = 0.06;

/// Errors larger than this count as octave errors, not tuning errors.
pub const OCTAVE_ERROR_CENTS: f64 = 600.0;

/// Pitch accuracy over a set of frames.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PitchStats {
    /// Mean absolute error in cents.
    pub mean_cents: Option<f64>,
    /// 95th-percentile absolute error in cents.
    pub p95_cents: Option<f64>,
    /// Maximum absolute error in cents.
    pub max_cents: Option<f64>,
    /// Frames more than 600 cents off.
    pub octave_errors: usize,
    /// Frames sung where silence/unvoiced was expected, or unvoiced where
    /// singing was expected.
    pub voicing_mismatches: usize,
    /// Frames scored.
    pub frames: usize,
}

/// Per-note pitch figures.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NotePitch {
    /// Mean absolute error.
    pub mean: Option<f64>,
    /// 95th percentile.
    pub p95: Option<f64>,
    /// Maximum.
    pub max: Option<f64>,
    /// Mean signed error.
    pub bias: Option<f64>,
    /// Octave errors.
    pub octave_errors: usize,
    /// Voiced fraction of steady frames.
    pub voiced_ratio: f64,
}

/// Frames of a note's steady part: inside the note, away from its edges.
fn steady_frames(n: &NoteTarget, f0: &F0Track) -> Vec<usize> {
    let edge = EDGE_SECS.min((n.end_sec - n.start_sec) / 4.0);
    let (a, b) = (n.start_sec + edge, n.end_sec - edge);
    (0..f0.hz.len())
        .filter(|&i| {
            let t = f0.t0 + i as f64 * f0.hop_secs;
            t >= a && t < b
        })
        .collect()
}

/// Signed errors in cents over a note's steady voiced frames, plus counts.
fn errors(n: &NoteTarget, target: &[f64], f0: &F0Track) -> (Vec<f64>, usize, usize) {
    let frames = steady_frames(n, f0);
    let mut errs = Vec::new();
    let mut voiced = 0;
    for &i in &frames {
        if f0.voiced[i] && f0.hz[i] > 0.0 {
            voiced += 1;
            if let Some(&t) = target.get(i) {
                errs.push(hz_to_cents(f64::from(f0.hz[i])) - t);
            }
        }
    }
    (errs, voiced, frames.len())
}

/// Signed error (seconds, cents) every second steady voiced frame of a
/// sung note, excluding octave errors.
pub fn contour(n: &NoteTarget, target: &[f64], f0: &F0Track) -> Vec<(f64, f64)> {
    if !n.sung {
        return Vec::new();
    }
    steady_frames(n, f0)
        .into_iter()
        .step_by(2)
        .filter(|&i| f0.voiced[i] && f0.hz[i] > 0.0 && i < target.len())
        .map(|i| {
            let e = hz_to_cents(f64::from(f0.hz[i])) - target[i];
            (f0.t0 + i as f64 * f0.hop_secs, e)
        })
        .filter(|(_, e)| e.abs() <= OCTAVE_ERROR_CENTS)
        .collect()
}

/// Pitch figures for one note.
pub fn note_pitch(n: &NoteTarget, target: &[f64], f0: &F0Track) -> NotePitch {
    if !n.sung {
        return NotePitch::default();
    }
    let (errs, voiced, total) = errors(n, target, f0);
    let octave_errors = errs.iter().filter(|e| e.abs() > OCTAVE_ERROR_CENTS).count();
    let tuned: Vec<f64> = errs
        .iter()
        .copied()
        .filter(|e| e.abs() <= OCTAVE_ERROR_CENTS)
        .collect();
    let mut abs: Vec<f64> = tuned.iter().map(|e| e.abs()).collect();
    let n_t = tuned.len() as f64;
    NotePitch {
        mean: (!abs.is_empty()).then(|| abs.iter().sum::<f64>() / n_t),
        bias: (!tuned.is_empty()).then(|| tuned.iter().sum::<f64>() / n_t),
        max: abs.iter().copied().reduce(f64::max),
        p95: percentile(&mut abs, 95.0),
        octave_errors,
        voiced_ratio: if total == 0 {
            0.0
        } else {
            voiced as f64 / total as f64
        },
    }
}

/// Pitch accuracy over all of a track's sung notes, plus voicing
/// mismatches over the whole track.
pub fn track_stats(notes: &[NoteTarget], target: &[f64], f0: &F0Track) -> PitchStats {
    let mut all = Vec::new();
    let mut octave = 0;
    for n in notes.iter().filter(|n| n.sung) {
        let (errs, _, _) = errors(n, target, f0);
        octave += errs.iter().filter(|e| e.abs() > OCTAVE_ERROR_CENTS).count();
        all.extend(
            errs.iter()
                .map(|e| e.abs())
                .filter(|e| *e <= OCTAVE_ERROR_CENTS),
        );
    }
    // Voicing: steady parts of sung notes should be voiced; frames well
    // outside any note should not be.
    let mut mismatches = 0;
    for (i, &v) in f0.voiced.iter().enumerate() {
        let t = f0.t0 + i as f64 * f0.hop_secs;
        let inside = notes.iter().find(|n| t >= n.start_sec && t < n.end_sec);
        let expected = match inside {
            Some(n) if n.sung => {
                let edge = EDGE_SECS.min((n.end_sec - n.start_sec) / 4.0);
                (t >= n.start_sec + edge && t < n.end_sec - edge).then_some(true)
            }
            Some(_) => Some(false),
            None => {
                let near = notes
                    .iter()
                    .any(|n| t > n.start_sec - 0.3 && t < n.end_sec + 0.3);
                (!near).then_some(false)
            }
        };
        if expected.is_some_and(|e| e != v) {
            mismatches += 1;
        }
    }
    let frames = all.len();
    PitchStats {
        mean_cents: (!all.is_empty()).then(|| all.iter().sum::<f64>() / frames as f64),
        max_cents: all.iter().copied().reduce(f64::max),
        p95_cents: percentile(&mut all, 95.0),
        octave_errors: octave,
        voicing_mismatches: mismatches,
        frames,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cantor_core::{NoteId, PhraseId};

    fn note(start: f64, end: f64, cents: f64) -> NoteTarget {
        NoteTarget {
            id: NoteId(1),
            phrase: PhraseId(1),
            lyric: "a".into(),
            start_sec: start,
            end_sec: end,
            sung: true,
            articulated: true,
            prev_cents: None,
            cents,
        }
    }

    fn track(cents: impl Fn(f64) -> Option<f64>, n: usize, hop: f64) -> F0Track {
        let mut hz = Vec::new();
        let mut voiced = Vec::new();
        for i in 0..n {
            match cents(i as f64 * hop) {
                Some(c) => {
                    hz.push((440.0 * 2f64.powf((c / 100.0 - 69.0) / 12.0)) as f32);
                    voiced.push(true);
                }
                None => {
                    hz.push(0.0);
                    voiced.push(false);
                }
            }
        }
        F0Track {
            hop_secs: hop,
            t0: 0.0,
            hz,
            voiced,
        }
    }

    #[test]
    fn measures_cents_error() {
        let hop = 0.01;
        let n = note(0.0, 1.0, 6000.0);
        let f0 = track(|t| (t < 1.0).then_some(6020.0), 120, hop);
        let target = vec![6000.0; 120];
        let p = note_pitch(&n, &target, &f0);
        assert!((p.mean.unwrap() - 20.0).abs() < 0.01);
        assert!((p.bias.unwrap() - 20.0).abs() < 0.01);
        assert_eq!(p.octave_errors, 0);
        assert!((p.voiced_ratio - 1.0).abs() < 1e-9);
    }

    #[test]
    fn separates_octave_errors() {
        let n = note(0.0, 1.0, 6000.0);
        let f0 = track(|t| Some(if t < 0.5 { 6000.0 } else { 7200.0 }), 100, 0.01);
        let p = note_pitch(&n, &vec![6000.0; 100], &f0);
        assert!(p.octave_errors > 30);
        assert!(p.max.unwrap() < 1.0);
    }
}
