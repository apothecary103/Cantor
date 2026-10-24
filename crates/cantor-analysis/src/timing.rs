//! Onset timing: where each note's vowel actually starts.

use crate::{F0Track, NoteReport, NoteTarget, hz_to_cents, percentile};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How far from the note onset a vowel onset is searched for.
pub const SEARCH_SECS: f64 = 0.15;

/// Onset timing over a track.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TimingStats {
    /// Mean absolute onset error in milliseconds.
    pub mean_abs_ms: Option<f64>,
    /// 95th-percentile absolute onset error in milliseconds.
    pub p95_abs_ms: Option<f64>,
    /// Largest absolute onset error in milliseconds.
    pub max_abs_ms: Option<f64>,
    /// Notes whose onset could be measured.
    pub measured: usize,
}

/// Detected vowel onset minus note onset in milliseconds.
///
/// After silence, the onset is where voicing begins and the frame energy
/// reaches half of the note's median energy. After a sung note (legato), it
/// is where the pitch crosses the midpoint between the two notes.
pub fn onset_error_ms(n: &NoteTarget, samples: &[f32], sr: u32, f0: &F0Track) -> Option<f64> {
    if !n.sung || !n.articulated {
        return None;
    }
    let lo = f0.frame_at(n.start_sec - SEARCH_SECS).unwrap_or(0);
    let hi = f0.frame_at((n.start_sec + SEARCH_SECS).min(n.end_sec))?;
    let time = |i: usize| f0.t0 + i as f64 * f0.hop_secs;

    if let Some(prev) = n.prev_cents
        && (prev - n.cents).abs() >= 50.0
        && f0.voiced.get(lo).copied().unwrap_or(false)
    {
        let mid = (prev + n.cents) / 2.0;
        let rising = n.cents > prev;
        for i in lo..=hi {
            if f0.voiced[i] {
                let c = hz_to_cents(f64::from(f0.hz[i]));
                if (rising && c >= mid) || (!rising && c <= mid) {
                    return Some((time(i) - n.start_sec) * 1000.0);
                }
            }
        }
        return None;
    }

    let energy = |i: usize| {
        let t = time(i);
        let a = ((t * f64::from(sr)) as usize).min(samples.len());
        let b = (((t + f0.hop_secs) * f64::from(sr)) as usize).min(samples.len());
        if a >= b {
            return 0.0;
        }
        samples[a..b]
            .iter()
            .map(|x| f64::from(*x).powi(2))
            .sum::<f64>()
            / (b - a) as f64
    };
    let body: Vec<usize> = (hi..f0.frame_at(n.end_sec).unwrap_or(hi)).collect();
    let mut body_e: Vec<f64> = body.iter().map(|&i| energy(i)).collect();
    let median = percentile(&mut body_e, 50.0)?;
    // Legato into a note at (nearly) the same pitch: the consonant shows as
    // an energy dip; the onset is the rise after the deepest dip.
    let from = if f0.voiced.get(lo).copied().unwrap_or(false) {
        (lo..=hi).min_by(|&a, &b| energy(a).total_cmp(&energy(b)))?
    } else {
        lo
    };
    (from..=hi)
        .find(|&i| f0.voiced[i] && energy(i) >= median / 2.0)
        .map(|i| (time(i) - n.start_sec) * 1000.0)
}

/// Summary over notes.
pub fn stats(notes: &[NoteReport]) -> TimingStats {
    let mut abs: Vec<f64> = notes
        .iter()
        .filter_map(|n| n.onset_error_ms)
        .map(f64::abs)
        .collect();
    let measured = abs.len();
    TimingStats {
        mean_abs_ms: (!abs.is_empty()).then(|| abs.iter().sum::<f64>() / measured as f64),
        max_abs_ms: abs.iter().copied().reduce(f64::max),
        p95_abs_ms: percentile(&mut abs, 95.0),
        measured,
    }
}
