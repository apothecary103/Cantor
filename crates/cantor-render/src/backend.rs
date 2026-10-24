//! The interface between the renderer and a synthesis backend.

use crate::phrase::{Phrase, StageSeeds};
use cantor_core::Diagnostic;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A backend's frame grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grid {
    /// Output sample rate in Hz.
    pub sample_rate: u32,
    /// Samples per frame.
    pub hop: usize,
    /// Mel bins per frame.
    pub mel_bins: usize,
}

impl Grid {
    /// Seconds per frame.
    pub fn frame_secs(&self) -> f64 {
        self.hop as f64 / f64::from(self.sample_rate)
    }
}

/// What the renderer asks a backend to predict for one phrase.
#[derive(Clone, Debug)]
pub struct PhraseInput<'a> {
    /// The phrase.
    pub phrase: &'a Phrase,
    /// Time of frame 0 in seconds (the phrase start minus head padding).
    pub t0: f64,
    /// Number of frames.
    pub frames: usize,
    /// Base pitch from notes, cents per frame.
    pub base_pitch: &'a [f64],
    /// Diffusion sampling steps.
    pub steps: u32,
    /// Noise seeds per stage.
    pub seeds: StageSeeds,
}

/// One placed phoneme.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Phone {
    /// Symbol in the backend's alphabet.
    pub symbol: String,
    /// Length in frames.
    pub frames: i64,
}

/// A backend's predictions for a phrase. Cached; must be deterministic.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Prediction {
    /// Phonemes covering all frames.
    pub phones: Vec<Phone>,
    /// Predicted pitch in cents per frame, if the backend predicts pitch.
    pub pitch: Option<Vec<f32>>,
    /// Predicted variance curves per parameter id, in the parameter's unit.
    pub variance: BTreeMap<String, Vec<f32>>,
    /// Findings, such as lyrics missing from the dictionary.
    pub diagnostics: Vec<Diagnostic>,
}

/// Inputs to the acoustic model.
#[derive(Clone, Debug)]
pub struct AcousticInput<'a> {
    /// Phonemes.
    pub phones: &'a [Phone],
    /// Reference pitch in Hz per frame: base pitch plus model prediction,
    /// without the user's pitch edits (they go to the vocoder).
    pub f0_ref_hz: &'a [f32],
    /// Final variance curves per parameter id.
    pub variance: &'a BTreeMap<String, Vec<f32>>,
    /// Sampling steps.
    pub steps: u32,
    /// Acoustic-model noise seed.
    pub seed: u64,
}

/// A backend failure.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct BackendError(pub String);

/// A synthesis backend: predictions, acoustic model and vocoder.
pub trait Backend: Send + Sync {
    /// Stable identifier, part of cache keys.
    fn id(&self) -> &str;
    /// Model version, part of cache keys.
    fn version(&self) -> &str;
    /// Frame grid.
    fn grid(&self) -> Grid;
    /// Curve parameters this backend honors (registry ids).
    fn params(&self) -> &[&'static str];
    /// Neutral value of a parameter when nothing is predicted or edited.
    fn param_default(&self, param: &str) -> f32;
    /// Valid range of a parameter's final value.
    fn param_range(&self, param: &str) -> (f32, f32);
    /// Phonemes, durations, pitch and variance for a phrase.
    fn predict(&self, input: &PhraseInput<'_>) -> Result<Prediction, BackendError>;
    /// Mel spectrogram, `frames × mel_bins`, row-major.
    fn acoustic(&self, input: &AcousticInput<'_>) -> Result<Vec<f32>, BackendError>;
    /// Audio from a mel spectrogram and f0 in Hz per frame.
    fn vocode(&self, mel: &[f32], f0_hz: &[f32]) -> Result<Vec<f32>, BackendError>;
}

/// Converts cents (MIDI × 100) to Hz.
pub fn cents_to_hz(cents: f64) -> f64 {
    440.0 * 2f64.powf((cents / 100.0 - 69.0) / 12.0)
}
