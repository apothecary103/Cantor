//! Render analysis (docs/spec/40-algorithms.md §11): what came out, as
//! numbers, so people and agents can judge a render without listening.
//!
//! Pitch is measured from an [`F0Track`] supplied by an extractor (RMVPE in
//! cantor-backends) against the score's intended pitch: notes, portamento
//! and vibrato, without the user's pitch-curve edits. An agent therefore
//! lowers pitch error by editing the pitch curve.

#![forbid(unsafe_code)]

pub mod artifacts;
pub mod loudness;
pub mod pitch;
pub mod timing;

use cantor_core::{NoteId, PhraseId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
}

/// What a note should sound like, for analysis.
#[derive(Clone, Debug, PartialEq)]
pub struct NoteTarget {
    /// The note.
    pub id: NoteId,
    /// Its phrase.
    pub phrase: PhraseId,
    /// Lyric, for reports.
    pub lyric: String,
    /// Start and end in seconds.
    pub start_sec: f64,
    /// End in seconds.
    pub end_sec: f64,
    /// Whether the note is meant to be sung (not a rest or breath).
    pub sung: bool,
    /// Pitch of the previous note in the same phrase, if adjacent.
    pub prev_cents: Option<f64>,
    /// Pitch in cents.
    pub cents: f64,
}

/// One track's rendered voice and score, for analysis.
#[derive(Clone, Debug)]
pub struct TrackInput<'a> {
    /// The track.
    pub track: TrackId,
    /// Mono samples of this track alone, starting at time 0.
    pub samples: &'a [f32],
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Notes in time order.
    pub notes: Vec<NoteTarget>,
    /// Intended pitch in cents on the F0 frame grid (score-derived).
    pub target_cents: Vec<f64>,
    /// Extracted F0 on the same grid.
    pub f0: F0Track,
    /// Times where phrase audio was joined.
    pub joins: Vec<f64>,
}

/// The full analysis of a render.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AnalysisReport {
    /// Per-track pitch and timing.
    pub tracks: Vec<TrackReport>,
    /// Loudness of the mix.
    pub loudness: loudness::Loudness,
    /// Detected artifacts across all tracks and the mix.
    pub artifacts: Vec<artifacts::Artifact>,
}

impl AnalysisReport {
    /// All note reports across tracks.
    pub fn notes(&self) -> impl Iterator<Item = &NoteReport> {
        self.tracks.iter().flat_map(|t| t.notes.iter())
    }
}

/// Analysis of one track.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TrackReport {
    /// The track.
    pub track: TrackId,
    /// Pitch accuracy over the track's sung frames.
    pub pitch: pitch::PitchStats,
    /// Onset timing over the track's notes.
    pub timing: timing::TimingStats,
    /// Per-note results.
    pub notes: Vec<NoteReport>,
}

/// Analysis of one note.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NoteReport {
    /// The note.
    pub id: NoteId,
    /// Its phrase.
    pub phrase: PhraseId,
    /// Lyric.
    pub lyric: String,
    /// Mean absolute pitch error in cents over the note's steady frames.
    pub pitch_error_mean: Option<f64>,
    /// 95th-percentile absolute pitch error in cents.
    pub pitch_error_p95: Option<f64>,
    /// Maximum absolute pitch error in cents.
    pub pitch_error_max: Option<f64>,
    /// Mean signed pitch error in cents (sharp is positive).
    pub pitch_bias: Option<f64>,
    /// Frames off by more than 600 cents (octave errors).
    pub octave_errors: usize,
    /// Fraction of the note's steady frames that are voiced.
    pub voiced_ratio: f64,
    /// Detected vowel onset minus note onset, in milliseconds.
    pub onset_error_ms: Option<f64>,
}

/// Analyzes rendered tracks and the final mix.
pub fn analyze(tracks: &[TrackInput<'_>], mix_stereo: &[f32], sample_rate: u32) -> AnalysisReport {
    let mut artifacts = artifacts::detect_mix(mix_stereo, sample_rate);
    let reports = tracks
        .iter()
        .map(|t| {
            let notes: Vec<NoteReport> = t
                .notes
                .iter()
                .map(|n| {
                    let p = pitch::note_pitch(n, &t.target_cents, &t.f0);
                    NoteReport {
                        id: n.id,
                        phrase: n.phrase,
                        lyric: n.lyric.clone(),
                        pitch_error_mean: p.mean,
                        pitch_error_p95: p.p95,
                        pitch_error_max: p.max,
                        pitch_bias: p.bias,
                        octave_errors: p.octave_errors,
                        voiced_ratio: p.voiced_ratio,
                        onset_error_ms: timing::onset_error_ms(n, t.samples, t.sample_rate, &t.f0),
                    }
                })
                .collect();
            artifacts.extend(artifacts::detect_track(t));
            TrackReport {
                track: t.track,
                pitch: pitch::track_stats(&t.notes, &t.target_cents, &t.f0),
                timing: timing::stats(&notes),
                notes,
            }
        })
        .collect();
    artifacts.sort_by(|a, b| a.at_sec.total_cmp(&b.at_sec));
    AnalysisReport {
        tracks: reports,
        loudness: loudness::measure(mix_stereo, sample_rate),
        artifacts,
    }
}

/// Percentile (0..=100) of values, by nearest rank.
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
