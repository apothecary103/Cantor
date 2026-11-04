//! The render report (docs/design.md "Render report"): numbers an agent
//! can act on without hearing the audio.
//!
//! Pitch is measured by running [`Yin`] on each track's rendered voice and
//! comparing every voiced frame (away from voicing edges) with the
//! track's final target pitch curve, and with the reference F0 when there
//! is one. Errors are in cents, `rendered − target` (+ = sharp).

use crate::artifacts::{self, Artifact};
use crate::{F0Track, ReferenceF0, Yin, loudness, percentile, rms_db};
use cantor_core::{NoteId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A target pitch curve: frame `i` at `start_sec + i · step_sec`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TargetPitch {
    /// Time of frame 0 in seconds.
    pub start_sec: f64,
    /// Seconds between frames.
    pub step_sec: f64,
    /// Intended pitch in MIDI cents.
    pub cents: Vec<f32>,
    /// Whether the frame is meant to be sung (inside a sung note).
    pub voiced: Vec<bool>,
}

impl TargetPitch {
    /// Intended pitch at `t`, linearly interpolated between voiced frames.
    pub fn at(&self, t: f64) -> Option<f64> {
        if self.step_sec <= 0.0 || self.cents.is_empty() {
            return None;
        }
        let x = (t - self.start_sec) / self.step_sec;
        if x < -0.5 || x > self.cents.len() as f64 - 0.5 {
            return None;
        }
        let i = (x.floor().max(0.0) as usize).min(self.cents.len() - 1);
        let j = (i + 1).min(self.cents.len() - 1);
        let v = |k: usize| self.voiced.get(k).copied().unwrap_or(false);
        let f = (x - i as f64).clamp(0.0, 1.0);
        match (v(i), v(j)) {
            (true, true) => {
                Some(f64::from(self.cents[i]) * (1.0 - f) + f64::from(self.cents[j]) * f)
            }
            (true, false) if f < 0.5 => Some(f64::from(self.cents[i])),
            (false, true) if f >= 0.5 => Some(f64::from(self.cents[j])),
            _ => None,
        }
    }
}

/// A sung note, as the synth placed it.
#[derive(Clone, Debug, PartialEq)]
pub struct NoteSpan {
    /// The note.
    pub note: NoteId,
    /// Lyric, for reports.
    pub lyric: String,
    /// Note start (vowel onset) in seconds.
    pub start_sec: f64,
    /// Note end in seconds.
    pub end_sec: f64,
    /// Index of its phrase in `cantor_core::phrases(track)`.
    pub phrase: usize,
    /// Whether it starts a new syllable (not a `+` extension), so its
    /// onset is measured.
    pub articulated: bool,
}

/// One track's rendered voice and score.
#[derive(Clone, Copy, Debug)]
pub struct VoiceInput<'a> {
    /// The track.
    pub track: TrackId,
    /// Mono samples of this track alone (before effects if possible),
    /// starting at project time 0.
    pub samples: &'a [f32],
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// The track's final pitch curve.
    pub target: &'a TargetPitch,
    /// Sung notes (not rests/breaths), in time order.
    pub notes: &'a [NoteSpan],
    /// Times (seconds) where sample audio was joined, checked for clicks.
    pub joins: &'a [f64],
    /// Reference F0 for this track, in project time.
    pub reference: Option<&'a ReferenceF0>,
}

/// Absolute-error percentiles in cents (or ms for onsets).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ErrorStats {
    /// Median absolute error.
    pub p50: f64,
    /// 95th-percentile absolute error.
    pub p95: f64,
    /// Median signed error (+ = sharp / late).
    pub bias: f64,
    /// Frames (or notes) measured.
    pub frames: usize,
}

impl ErrorStats {
    /// Stats of signed errors; `None` if empty.
    pub fn of(signed: &[f64]) -> Option<Self> {
        if signed.is_empty() {
            return None;
        }
        let mut abs: Vec<f64> = signed.iter().map(|e| e.abs()).collect();
        let mut s = signed.to_vec();
        Some(Self {
            p50: percentile(&mut abs, 50.0)?,
            p95: percentile(&mut abs, 95.0)?,
            bias: percentile(&mut s, 50.0)?,
            frames: signed.len(),
        })
    }
}

/// A note whose lyric found no alias (filled by the synth).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MissingAlias {
    /// The note.
    pub note: NoteId,
    /// Its lyric.
    pub lyric: String,
}

/// The phrase's least accurate note.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WorstNote {
    /// The note.
    pub note: NoteId,
    /// Lyric.
    pub lyric: String,
    /// Note start in seconds.
    pub start_sec: f64,
    /// 95th-percentile absolute pitch error vs the score, cents.
    pub p95: f64,
    /// Median signed error vs the score, cents (+ = sharp).
    pub bias: f64,
    /// 95th-percentile absolute error vs the reference, cents.
    pub ref_p95: Option<f64>,
}

/// Per-phrase results.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PhraseReport {
    /// The track.
    pub track: TrackId,
    /// Phrase index within the track.
    pub index: usize,
    /// First note start, seconds.
    pub start_sec: f64,
    /// Last note end, seconds.
    pub end_sec: f64,
    /// Median absolute pitch error vs the score, cents.
    pub pitch_p50: Option<f64>,
    /// 95th-percentile absolute pitch error vs the score, cents.
    pub pitch_p95: Option<f64>,
    /// 95th-percentile absolute pitch error vs the reference, cents.
    pub ref_p95: Option<f64>,
    /// 95th-percentile absolute onset error, ms.
    pub onset_p95_ms: Option<f64>,
    /// Fraction of the notes' steady frames that came out voiced; well
    /// below 1 means silence or noise where singing was expected.
    pub voiced_ratio: f64,
    /// The note with the largest pitch p95 vs the score.
    pub worst_note: Option<WorstNote>,
}

/// The render report.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RenderReport {
    /// Wall-clock render time in ms (filled by the synth).
    pub render_ms: f64,
    /// Integrated loudness, LUFS (`null` for silence).
    pub lufs: Option<f64>,
    /// True peak, dBTP.
    pub true_peak_db: Option<f64>,
    /// Clipped samples in the mix (any channel).
    pub clipped: u64,
    /// Pitch error vs the score over all tracks (`null` if nothing voiced
    /// was measured).
    pub pitch_error: Option<ErrorStats>,
    /// Pitch error vs the reference, when there is one.
    pub ref_pitch_error: Option<ErrorStats>,
    /// Onset error in ms (detected vowel onset − note start).
    #[serde(default)]
    pub onset_error_ms: Option<ErrorStats>,
    /// Notes whose lyric found no alias (filled by the synth).
    pub missing_aliases: Vec<MissingAlias>,
    /// Per-phrase results, by track then time.
    pub phrases: Vec<PhraseReport>,
    /// Clipping, join clicks and unexpected silence.
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
}

/// Frames within this distance of a note edge are excluded from the
/// voiced-ratio check (consonants and transitions live there).
pub const EDGE_SECS: f64 = 0.06;
/// How far around a note start its onset is searched for.
pub const ONSET_SEARCH_SECS: f64 = 0.15;

/// F0 of a rendered voice, as the report measures it.
pub fn voice_f0(samples: &[f32], sample_rate: u32) -> F0Track {
    Yin::for_reference().track(samples, sample_rate)
}

/// Frames usable for pitch error: voiced with voiced neighbours.
fn scored(f0: &F0Track, i: usize) -> Option<f64> {
    let ok = |k: usize| f0.voiced.get(k).copied().unwrap_or(false);
    (i > 0 && ok(i - 1) && ok(i + 1))
        .then(|| f0.cents(i))
        .flatten()
}

/// Signed pitch errors of `f0` vs `target` over scored frames.
pub fn pitch_errors(f0: &F0Track, target: &TargetPitch) -> Vec<f64> {
    (0..f0.hz.len())
        .filter_map(|i| Some(scored(f0, i)? - target.at(f0.time(i))?))
        .collect()
}

/// Signed pitch errors of `f0` vs a reference over scored frames.
pub fn ref_pitch_errors(f0: &F0Track, reference: &ReferenceF0) -> Vec<f64> {
    (0..f0.hz.len())
        .filter_map(|i| Some(scored(f0, i)? - f64::from(reference.cents_at(f0.time(i))?)))
        .collect()
}

/// Detected vowel onset minus note start, ms. After a note at a
/// different pitch (legato), the onset is where the rendered pitch crosses
/// midway between the two target pitches; otherwise where the voice
/// becomes voiced at ≥ half (−6 dB) of the note body's median energy.
pub fn onset_error_ms(n: &NoteSpan, samples: &[f32], sr: u32, f0: &F0Track) -> Option<f64> {
    if !n.articulated {
        return None;
    }
    let lo = f0.frame_at((n.start_sec - ONSET_SEARCH_SECS).max(0.0))?;
    let hi = f0.frame_at((n.start_sec + ONSET_SEARCH_SECS).min(n.end_sec))?;
    if hi <= lo {
        return None;
    }
    let ms = |i: usize| (f0.time(i) - n.start_sec) * 1000.0;
    let srf = f64::from(sr);
    let energy = |i: usize| {
        let a = ((f0.time(i) * srf) as usize).min(samples.len());
        let b = (((f0.time(i) + f0.hop_secs) * srf) as usize).min(samples.len());
        rms_db(&samples[a..b])
    };
    if f0.voiced[lo] {
        // Legato: pitch crossing if the pitch changes.
        let before = f0.cents(lo);
        let end = f0.frame_at(n.end_sec).unwrap_or(f0.hz.len() - 1);
        let mut body: Vec<f64> = (hi..=end).filter_map(|i| f0.cents(i)).collect();
        let after = percentile(&mut body, 50.0);
        if let (Some(b), Some(a)) = (before, after)
            && (a - b).abs() >= 50.0
        {
            let mid = 0.5 * (a + b);
            return (lo..=hi)
                .find(|&i| f0.cents(i).is_some_and(|c| (c - mid) * (a - b) > 0.0))
                .map(ms);
        }
        // Same pitch: the consonant shows as an energy dip; onset is the
        // rise after the deepest dip.
        let dip = (lo..=hi).min_by(|&x, &y| energy(x).total_cmp(&energy(y)))?;
        let floor = energy(dip);
        return (dip..=hi)
            .find(|&i| f0.voiced[i] && energy(i) >= floor + 6.0)
            .map(ms);
    }
    let end = f0.frame_at(n.end_sec).unwrap_or(f0.hz.len() - 1);
    let mut body: Vec<f64> = (hi..end.max(hi + 1)).map(energy).collect();
    let median = percentile(&mut body, 50.0)?;
    (lo..=hi)
        .find(|&i| f0.voiced[i] && energy(i) >= median - 6.0)
        .map(ms)
}

/// Computes the report from the final mix (interleaved, `channels`
/// channels) and each track's voice. `render_ms` and `missing_aliases`
/// are left for the caller.
pub fn render_report(
    mix: &[f32],
    channels: u32,
    sample_rate: u32,
    voices: &[VoiceInput<'_>],
) -> RenderReport {
    let loud = loudness::measure(mix, channels, sample_rate);
    let mut artifacts = artifacts::clipping(mix, channels as usize, sample_rate, 50);
    let (mut all, mut all_ref, mut all_onset) = (Vec::new(), Vec::new(), Vec::new());
    let mut phrases = Vec::new();
    for v in voices {
        let f0 = voice_f0(v.samples, v.sample_rate);
        artifacts.extend(artifacts::discontinuities(
            v.samples,
            v.sample_rate,
            v.joins,
            v.track,
        ));
        let spans: Vec<(f64, f64)> = v.notes.iter().map(|n| (n.start_sec, n.end_sec)).collect();
        artifacts.extend(artifacts::silences(
            v.samples,
            v.sample_rate,
            &spans,
            v.track,
        ));

        // Per-note frame errors.
        struct Acc {
            err: Vec<f64>,
            refe: Vec<f64>,
            steady: usize,
            voiced: usize,
        }
        let mut acc: Vec<Acc> = v
            .notes
            .iter()
            .map(|_| Acc {
                err: Vec::new(),
                refe: Vec::new(),
                steady: 0,
                voiced: 0,
            })
            .collect();
        let mut order: Vec<usize> = (0..v.notes.len()).collect();
        order.sort_by(|&a, &b| v.notes[a].start_sec.total_cmp(&v.notes[b].start_sec));
        let starts: Vec<f64> = order.iter().map(|&k| v.notes[k].start_sec).collect();
        for i in 0..f0.hz.len() {
            let t = f0.time(i);
            let p = starts.partition_point(|&s| s <= t);
            let Some(k) = (0..p)
                .rev()
                .take(4)
                .map(|j| order[j])
                .find(|&k| t < v.notes[k].end_sec)
            else {
                continue;
            };
            let n = &v.notes[k];
            let target = v.target.at(t);
            let edge = EDGE_SECS.min((n.end_sec - n.start_sec) / 4.0);
            if target.is_some() && t >= n.start_sec + edge && t < n.end_sec - edge {
                acc[k].steady += 1;
                acc[k].voiced += usize::from(f0.voiced[i]);
            }
            let Some(c) = scored(&f0, i) else { continue };
            if let Some(tc) = target {
                acc[k].err.push(c - tc);
            }
            if let Some(r) = v.reference.and_then(|r| r.cents_at(t)) {
                acc[k].refe.push(c - f64::from(r));
            }
        }
        let onsets: Vec<Option<f64>> = v
            .notes
            .iter()
            .map(|n| onset_error_ms(n, v.samples, v.sample_rate, &f0))
            .collect();

        let mut idx: Vec<usize> = v.notes.iter().map(|n| n.phrase).collect();
        idx.sort_unstable();
        idx.dedup();
        for p in idx {
            let members: Vec<usize> = (0..v.notes.len())
                .filter(|&k| v.notes[k].phrase == p)
                .collect();
            let err: Vec<f64> = members.iter().flat_map(|&k| acc[k].err.clone()).collect();
            let refe: Vec<f64> = members.iter().flat_map(|&k| acc[k].refe.clone()).collect();
            let ons: Vec<f64> = members.iter().filter_map(|&k| onsets[k]).collect();
            let (steady, voiced) = members.iter().fold((0, 0), |(s, vv), &k| {
                (s + acc[k].steady, vv + acc[k].voiced)
            });
            let worst = members
                .iter()
                .filter(|&&k| acc[k].err.len() >= 3)
                .filter_map(|&k| Some((k, ErrorStats::of(&acc[k].err)?)))
                .max_by(|a, b| a.1.p95.total_cmp(&b.1.p95))
                .map(|(k, s)| WorstNote {
                    note: v.notes[k].note,
                    lyric: v.notes[k].lyric.clone(),
                    start_sec: v.notes[k].start_sec,
                    p95: s.p95,
                    bias: s.bias,
                    ref_p95: ErrorStats::of(&acc[k].refe).map(|r| r.p95),
                });
            let es = ErrorStats::of(&err);
            phrases.push(PhraseReport {
                track: v.track,
                index: p,
                start_sec: members
                    .iter()
                    .map(|&k| v.notes[k].start_sec)
                    .fold(f64::INFINITY, f64::min),
                end_sec: members
                    .iter()
                    .map(|&k| v.notes[k].end_sec)
                    .fold(f64::NEG_INFINITY, f64::max),
                pitch_p50: es.as_ref().map(|s| s.p50),
                pitch_p95: es.map(|s| s.p95),
                ref_p95: ErrorStats::of(&refe).map(|s| s.p95),
                onset_p95_ms: ErrorStats::of(&ons).map(|s| s.p95),
                voiced_ratio: if steady == 0 {
                    0.0
                } else {
                    voiced as f64 / steady as f64
                },
                worst_note: worst,
            });
            all.extend(err);
            all_ref.extend(refe);
            all_onset.extend(ons);
        }
    }
    artifacts.sort_by(|a, b| a.at_sec.total_cmp(&b.at_sec));
    RenderReport {
        render_ms: 0.0,
        lufs: loud.integrated_lufs,
        true_peak_db: loud.true_peak_dbtp,
        clipped: artifacts::clipped_samples(mix),
        pitch_error: ErrorStats::of(&all),
        ref_pitch_error: ErrorStats::of(&all_ref),
        onset_error_ms: ErrorStats::of(&all_onset),
        missing_aliases: Vec::new(),
        phrases,
        artifacts,
    }
}
