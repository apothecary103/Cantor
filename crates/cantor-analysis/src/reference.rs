//! Reference-vocal analysis: F0 in cents on a 10 ms grid, a 50 ms RMS
//! envelope and onsets. Needs an isolated (a cappella) vocal; full mixes
//! give poor F0 (docs/tuning.md §3).

use crate::onset::onsets;
use crate::{F0Track, Yin, hz_to_cents, rms_db};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Analysis of a reference vocal. Frame `i` is at
/// `start_sec + i · step_sec`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ReferenceF0 {
    /// Time of frame 0 in seconds (project time).
    pub start_sec: f64,
    /// Seconds between frames.
    pub step_sec: f64,
    /// Pitch in MIDI cents per frame; `null` where unvoiced.
    pub cents: Vec<Option<f32>>,
    /// RMS level in dBFS over 50 ms around each frame (−120 = silence).
    pub rms_db: Vec<f32>,
    /// Note/syllable onsets in seconds (project time), ascending.
    #[serde(default)]
    pub onsets: Vec<f64>,
}

impl ReferenceF0 {
    /// Frames.
    pub fn len(&self) -> usize {
        self.cents.len()
    }

    /// Whether there are no frames.
    pub fn is_empty(&self) -> bool {
        self.cents.is_empty()
    }

    /// Time of frame `i` in seconds.
    pub fn time(&self, i: usize) -> f64 {
        self.start_sec + i as f64 * self.step_sec
    }

    /// The frame nearest `t`, if inside.
    pub fn frame_at(&self, t: f64) -> Option<usize> {
        if self.step_sec <= 0.0 {
            return None;
        }
        let i = ((t - self.start_sec) / self.step_sec).round();
        (i >= 0.0 && (i as usize) < self.cents.len()).then_some(i as usize)
    }

    /// Pitch at `t` (nearest frame), if voiced.
    pub fn cents_at(&self, t: f64) -> Option<f32> {
        self.frame_at(t).and_then(|i| self.cents[i])
    }

    /// The same analysis shifted so frame 0 is at `start_sec`.
    pub fn shifted(mut self, start_sec: f64) -> Self {
        let d = start_sec - self.start_sec;
        self.start_sec = start_sec;
        for o in &mut self.onsets {
            *o += d;
        }
        self
    }

    /// Builds a reference from an F0 track (no envelope or onsets).
    pub fn from_track(f0: &F0Track) -> Self {
        Self {
            start_sec: f0.t0,
            step_sec: f0.hop_secs,
            cents: (0..f0.hz.len())
                .map(|i| f0.cents(i).map(|c| c as f32))
                .collect(),
            rms_db: vec![0.0; f0.hz.len()],
            onsets: Vec::new(),
        }
    }
}

/// Analyzes mono `samples` (downmix stereo first) with the default
/// 10 ms [`Yin`].
pub fn analyze_reference(samples: &[f32], sample_rate: u32) -> ReferenceF0 {
    analyze_reference_with(&Yin::for_reference(), samples, sample_rate)
}

/// [`analyze_reference`] with a specific extractor configuration.
pub fn analyze_reference_with(yin: &Yin, samples: &[f32], sample_rate: u32) -> ReferenceF0 {
    let f0 = yin.track(samples, sample_rate);
    let sr = f64::from(sample_rate.max(1));
    let half = (0.025 * sr) as usize;
    let rms = (0..f0.hz.len())
        .map(|i| {
            let c = (f0.time(i) * sr).round() as usize;
            let a = c.saturating_sub(half).min(samples.len());
            let b = (c + half).min(samples.len());
            rms_db(&samples[a..b]) as f32
        })
        .collect();
    ReferenceF0 {
        start_sec: 0.0,
        step_sec: f0.hop_secs,
        cents: (0..f0.hz.len())
            .map(|i| f0.voiced[i].then(|| hz_to_cents(f64::from(f0.hz[i])) as f32))
            .collect(),
        rms_db: rms,
        onsets: onsets(samples, sample_rate),
    }
}
