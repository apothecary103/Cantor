//! Cantor's DSP analysis (docs/design.md "Analysis"): F0 extraction,
//! reference-vocal analysis and transcription, and the render report that
//! lets people and agents judge a render without listening.
//!
//! - [`yin`]: the [`Yin`] F0 extractor (YIN + Viterbi candidate tracking),
//!   behind the object-safe [`F0Extractor`] trait. cantor-synth uses it for
//!   voicebank samples that have no `.frq`.
//! - [`reference`]: [`analyze_reference`] → [`ReferenceF0`] (10 ms F0 in
//!   cents, RMS envelope, onsets) and [`onset`] detection.
//! - [`transcribe`](mod@transcribe): reference F0 → [`cantor_core::Note`]s
//!   per docs/tuning.md §3.
//! - [`report`]: [`RenderReport`] (pitch error vs score and reference,
//!   onset timing, loudness, clipping, join clicks) and
//!   [`render_report`].
//! - [`loudness`]: BS.1770 integrated loudness and true peak.
//!
//! Pitch is in MIDI cents throughout (`key · 100`; 6900 = A4 = 440 Hz).
//! Everything here is deterministic and model-free.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod artifacts;
pub mod loudness;
pub mod onset;
pub mod reference;
pub mod report;
pub mod transcribe;
pub mod yin;

pub use reference::{ReferenceF0, analyze_reference, analyze_reference_with};
pub use report::{
    ErrorStats, MissingAlias, NoteSpan, PhraseReport, RenderReport, TargetPitch, VoiceInput,
    WorstNote, render_report,
};
pub use transcribe::{TranscribeOptions, Transcription, transcribe, transcribe_detailed};
pub use yin::{Yin, YinConfig};

/// Fundamental frequency per frame from an extractor.
#[derive(Clone, Debug, PartialEq)]
pub struct F0Track {
    /// Seconds between frames.
    pub hop_secs: f64,
    /// Time of frame 0 in seconds.
    pub t0: f64,
    /// F0 in Hz per frame (meaningful only where voiced).
    pub hz: Vec<f32>,
    /// Voicing per frame.
    pub voiced: Vec<bool>,
}

impl F0Track {
    /// Index of the frame nearest `t`, if inside the track.
    pub fn frame_at(&self, t: f64) -> Option<usize> {
        let i = ((t - self.t0) / self.hop_secs).round();
        (i >= 0.0 && (i as usize) < self.hz.len()).then_some(i as usize)
    }

    /// Time of frame `i` in seconds.
    pub fn time(&self, i: usize) -> f64 {
        self.t0 + i as f64 * self.hop_secs
    }

    /// Pitch of frame `i` in cents, if voiced.
    pub fn cents(&self, i: usize) -> Option<f64> {
        (self.voiced.get(i).copied().unwrap_or(false) && self.hz[i] > 0.0)
            .then(|| hz_to_cents(f64::from(self.hz[i])))
    }
}

/// Estimates F0 from audio. Object-safe, so extractors can be swapped
/// behind `&dyn F0Extractor` / `Arc<dyn F0Extractor>`.
pub trait F0Extractor: Send + Sync {
    /// Name and version, for reports and cache keys.
    fn id(&self) -> &str;
    /// F0 of mono `samples` at `sample_rate`, starting at time 0.
    fn extract(&self, samples: &[f32], sample_rate: u32) -> Result<F0Track, String>;
}

/// Percentile (0..=100) of values, by nearest rank (sorts them).
pub(crate) fn percentile(values: &mut [f64], p: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let rank = ((p / 100.0) * values.len() as f64).ceil() as usize;
    Some(values[rank.clamp(1, values.len()) - 1])
}

/// Hz to cents (MIDI × 100).
pub fn hz_to_cents(hz: f64) -> f64 {
    6900.0 + 1200.0 * (hz / 440.0).log2()
}

/// Cents (MIDI × 100) to Hz.
pub fn cents_to_hz(cents: f64) -> f64 {
    440.0 * ((cents - 6900.0) / 1200.0).exp2()
}

/// RMS in dBFS of a sample range (−120 for silence).
pub(crate) fn rms_db(x: &[f32]) -> f64 {
    if x.is_empty() {
        return -120.0;
    }
    let ms = x.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / x.len() as f64;
    if ms > 1e-12 {
        10.0 * ms.log10()
    } else {
        -120.0
    }
}
