//! The renderer: phrases → predictions → mel → audio, through the cache.

use crate::backend::{AcousticInput, Backend, PhraseInput, Prediction, cents_to_hz};
use crate::cache::{DiskCache, Kind};
use crate::key::{Key, KeyBuilder, audio_hash};
use crate::mix::{PlacedAudio, mix};
use crate::phrase::{Phrase, split};
use crate::pitch::base_pitch;
use crate::rng::Rng;
use cantor_analysis::{AnalysisReport, F0Extractor, F0Track, NoteTarget, TrackInput};
use cantor_core::{Diagnostic, EntityRef, ParamId, PhraseId, Project, TempoMap, Track, TrackId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Silence rendered before a phrase's first note, room for leading consonants.
pub const HEAD_SECS: f64 = 0.5;
/// Silence rendered after a phrase's last note.
pub const TAIL_SECS: f64 = 0.3;

/// Render quality.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    /// Few sampling steps, for editing.
    Preview,
    /// Many sampling steps, for export.
    Final,
}

impl Quality {
    /// Diffusion sampling steps.
    pub fn steps(self) -> u32 {
        match self {
            Quality::Preview => 10,
            Quality::Final => 20,
        }
    }
}

/// What happened to a phrase during a render.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The phrase audio came from the cache.
    CacheHit,
    /// The mel came from the cache; only the vocoder ran (pitch-only edit).
    VocoderFastPath,
    /// The acoustic model and the vocoder ran.
    Full,
}

/// One phrase in a render trace.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PhraseTrace {
    /// The track.
    pub track: TrackId,
    /// The phrase.
    pub phrase: PhraseId,
    /// What ran.
    pub outcome: Outcome,
    /// Whether the predictions came from the cache.
    pub prediction_cached: bool,
    /// The phrase's audio cache key.
    pub audio_key: String,
}

/// What a render did, phrase by phrase.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RenderTrace {
    /// Phrases in render order.
    pub phrases: Vec<PhraseTrace>,
}

impl RenderTrace {
    /// Fraction of phrases served entirely from the cache.
    pub fn cache_hit_rate(&self) -> f64 {
        if self.phrases.is_empty() {
            return 1.0;
        }
        let hits = self
            .phrases
            .iter()
            .filter(|p| p.outcome == Outcome::CacheHit)
            .count();
        hits as f64 / self.phrases.len() as f64
    }
}

/// Rendered audio.
#[derive(Clone, Debug)]
pub struct RenderOutput {
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Interleaved stereo samples.
    pub samples: Vec<f32>,
    /// BLAKE3 of the samples.
    pub audio_hash: Key,
    /// Per-phrase trace.
    pub trace: RenderTrace,
    /// Everything the render could not honor.
    pub diagnostics: Vec<Diagnostic>,
    /// Measurements of what was rendered, if the renderer has an F0
    /// extractor.
    pub analysis: Option<AnalysisReport>,
}

/// Why a render failed.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// A backend failed.
    #[error("backend failed on {phrase}: {source}")]
    Backend {
        /// The phrase.
        phrase: PhraseId,
        /// The failure.
        source: crate::backend::BackendError,
    },
    /// The cache could not be written.
    #[error("render cache: {0}")]
    Cache(#[from] std::io::Error),
    /// Pitch extraction for the analysis failed.
    #[error("analysis failed: {0}")]
    Analysis(String),
}

/// Renders projects with one backend and one cache.
pub struct Renderer {
    backend: Arc<dyn Backend>,
    cache: DiskCache,
    analyzer: Option<Arc<dyn F0Extractor>>,
}

impl std::fmt::Debug for Renderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Renderer")
            .field("backend", &self.backend.id())
            .field("cache", &self.cache)
            .finish()
    }
}

impl Renderer {
    /// Creates a renderer.
    pub fn new(backend: Arc<dyn Backend>, cache: DiskCache) -> Self {
        Self {
            backend,
            cache,
            analyzer: None,
        }
    }

    /// Adds analysis to every render, extracting pitch with `extractor`.
    pub fn with_analyzer(mut self, extractor: Arc<dyn F0Extractor>) -> Self {
        self.analyzer = Some(extractor);
        self
    }

    /// The render cache.
    pub fn cache(&self) -> &DiskCache {
        &self.cache
    }

    /// Renders every audible track of a project.
    pub fn render(&self, project: &Project, quality: Quality) -> Result<RenderOutput, RenderError> {
        self.render_range(project, quality, None)
    }

    /// Renders only the phrases that overlap `range` (all phrases when
    /// `None`). Audio still starts at time 0.
    pub fn render_range(
        &self,
        project: &Project,
        quality: Quality,
        range: Option<cantor_core::TickRange>,
    ) -> Result<RenderOutput, RenderError> {
        let grid = self.backend.grid();
        let sr = grid.sample_rate;
        let tempo = project.tempo();
        let mut diagnostics = Vec::new();
        let mut trace = RenderTrace::default();
        let any_solo = project.tracks.iter().any(|t| t.mix.solo);

        let mut tracks: Vec<(&Track, Vec<f32>, Vec<Phrase>)> = Vec::new();
        for track in &project.tracks {
            if track.mix.mute || (any_solo && !track.mix.solo) {
                continue;
            }
            check_track_support(track, self.backend.as_ref(), &mut diagnostics);
            let mut phrases = split(project, track, &tempo);
            if let Some(r) = range {
                phrases.retain(|p| {
                    let start = p.notes[0].start;
                    let end = p.notes.iter().map(|n| n.end).max().unwrap_or(start);
                    start < r.end && end > r.start
                });
            }
            let mut rendered = Vec::with_capacity(phrases.len());
            for phrase in &phrases {
                let (audio, t0, pt) =
                    self.render_phrase(track, phrase, &tempo, quality, &mut diagnostics)?;
                trace.phrases.push(pt);
                rendered.push((t0, audio, phrase));
            }
            let end = rendered
                .iter()
                .map(|(t0, a, _)| t0 + a.len() as f64 / f64::from(sr))
                .fold(0.0, f64::max);
            let placed: Vec<PlacedAudio<'_>> = rendered
                .iter()
                .enumerate()
                .map(|(i, (t0, audio, phrase))| {
                    let boundary = |a: &Phrase, b: &Phrase| (a.end_sec() + b.start_sec()) / 2.0;
                    PlacedAudio {
                        start_sec: *t0,
                        samples: audio,
                        lo_sec: i.checked_sub(1).map(|j| boundary(rendered[j].2, phrase)),
                        hi_sec: rendered.get(i + 1).map(|n| boundary(phrase, n.2)),
                    }
                })
                .collect();
            let len = (end * f64::from(sr)).ceil() as usize;
            let mono = mix(&placed, sr, len);
            drop(placed);
            drop(rendered);
            tracks.push((track, mono, phrases));
        }

        let len = tracks.iter().map(|(_, a, _)| a.len()).max().unwrap_or(0);
        let mut samples = vec![0.0f32; len * 2];
        for (track, mono, _) in &tracks {
            let gain = 10f64.powf(track.mix.gain_db / 20.0);
            // Equal-power pan.
            let angle = (track.mix.pan.clamp(-1.0, 1.0) + 1.0) * std::f64::consts::FRAC_PI_4;
            let (gl, gr) = ((gain * angle.cos()) as f32, (gain * angle.sin()) as f32);
            for (i, s) in mono.iter().enumerate() {
                samples[2 * i] += s * gl;
                samples[2 * i + 1] += s * gr;
            }
        }
        let analysis = match &self.analyzer {
            Some(x) => Some(self.analyze(x.as_ref(), &tempo, &tracks, &samples, sr)?),
            None => None,
        };
        Ok(RenderOutput {
            sample_rate: sr,
            audio_hash: audio_hash(&samples),
            samples,
            trace,
            diagnostics,
            analysis,
        })
    }

    fn analyze(
        &self,
        extractor: &dyn F0Extractor,
        tempo: &TempoMap,
        tracks: &[(&Track, Vec<f32>, Vec<Phrase>)],
        mix: &[f32],
        sr: u32,
    ) -> Result<AnalysisReport, RenderError> {
        let mut inputs = Vec::with_capacity(tracks.len());
        for (track, mono, phrases) in tracks {
            let key = KeyBuilder::new("f0")
                .str("extractor", extractor.id())
                .u64("sample_rate", u64::from(sr))
                .key("audio", &audio_hash(mono))
                .finish();
            let f0 = match self
                .cache
                .get(Kind::Prediction, &key)
                .and_then(|b| serde_json::from_slice::<CachedF0>(&b).ok())
            {
                Some(c) => c.into_track(),
                None => {
                    let t = extractor.extract(mono, sr).map_err(RenderError::Analysis)?;
                    let json =
                        serde_json::to_vec(&CachedF0::from_track(&t)).expect("f0 serializes");
                    self.cache.put(Kind::Prediction, &key, &json)?;
                    t
                }
            };
            let mut target = vec![0.0; f0.hz.len()];
            let mut notes = Vec::new();
            for ph in phrases {
                let a = f0.frame_at(ph.start_sec() - 0.1).unwrap_or(0);
                let b = f0.frame_at(ph.end_sec() + 0.1).unwrap_or(f0.hz.len());
                if b > a {
                    let t0 = f0.t0 + a as f64 * f0.hop_secs;
                    let base = base_pitch(&ph.notes, t0, f0.hop_secs, b - a, ph.seed);
                    target[a..b].copy_from_slice(&base);
                }
                for (i, n) in ph.notes.iter().enumerate() {
                    let prev = i
                        .checked_sub(1)
                        .map(|j| &ph.notes[j])
                        .filter(|p| n.start_sec - p.end_sec < 0.05);
                    notes.push(NoteTarget {
                        id: n.id,
                        phrase: ph.id,
                        lyric: n.lyric.clone(),
                        start_sec: n.start_sec,
                        end_sec: n.end_sec,
                        sung: is_sung(&n.lyric),
                        articulated: !n.lyric.trim().starts_with('+'),
                        prev_cents: prev.map(|p| p.pitch_cents),
                        cents: n.pitch_cents,
                    });
                }
            }
            let joins = phrases
                .windows(2)
                .map(|w| (w[0].end_sec() + w[1].start_sec()) / 2.0)
                .collect();
            inputs.push(TrackInput {
                track: track.id,
                samples: mono,
                sample_rate: sr,
                notes,
                target_cents: target,
                f0,
                joins,
                tempo: tempo.clone(),
            });
        }
        Ok(cantor_analysis::analyze(&inputs, mix, sr))
    }
}

/// Whether a lyric is sung (not a rest or breath).
fn is_sung(lyric: &str) -> bool {
    !matches!(
        lyric.trim(),
        "" | "-" | "SP" | "R" | "r" | "AP" | "br" | "breath"
    )
}

/// An F0 track in the cache.
#[derive(Serialize, Deserialize)]
struct CachedF0 {
    hop_secs: f64,
    t0: f64,
    hz: Vec<f32>,
    voiced: Vec<bool>,
}

impl CachedF0 {
    fn from_track(t: &F0Track) -> Self {
        Self {
            hop_secs: t.hop_secs,
            t0: t.t0,
            hz: t.hz.clone(),
            voiced: t.voiced.clone(),
        }
    }

    fn into_track(self) -> F0Track {
        F0Track {
            hop_secs: self.hop_secs,
            t0: self.t0,
            hz: self.hz,
            voiced: self.voiced,
        }
    }
}

impl Renderer {
    fn render_phrase(
        &self,
        track: &Track,
        phrase: &Phrase,
        tempo: &TempoMap,
        quality: Quality,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Result<(Vec<f32>, f64, PhraseTrace), RenderError> {
        let b = self.backend.as_ref();
        let grid = b.grid();
        let fs = grid.frame_secs();
        let t0 = phrase.start_sec() - HEAD_SECS;
        let frames = ((phrase.end_sec() + TAIL_SECS - t0) / fs).ceil() as usize;
        let steps = quality.steps();
        let base = base_pitch(&phrase.notes, t0, fs, frames, phrase.seed);
        let backend_err = |source| RenderError::Backend {
            phrase: phrase.id,
            source,
        };

        // Predictions depend only on the phrase, its window and the seed.
        let pred_key = prediction_key(b, phrase, t0, frames, steps);
        let (pred, prediction_cached) = match self
            .cache
            .get(Kind::Prediction, &pred_key)
            .and_then(|bytes| serde_json::from_slice::<Prediction>(&bytes).ok())
        {
            Some(p) => (p, true),
            None => {
                let p = b
                    .predict(&PhraseInput {
                        phrase,
                        t0,
                        frames,
                        base_pitch: &base,
                        steps,
                        seeds: phrase.seeds,
                    })
                    .map_err(backend_err)?;
                let json = serde_json::to_vec(&p).expect("predictions serialize");
                self.cache.put(Kind::Prediction, &pred_key, &json)?;
                (p, false)
            }
        };
        diagnostics.extend(pred.diagnostics.iter().cloned());
        if !phrase.unsupported_dims.is_empty() {
            diagnostics.push(Diagnostic::warning(
                "render.retake_dim_unsupported",
                format!(
                    "{} cannot vary {:?}; the take's other dimensions are applied",
                    b.id(),
                    phrase.unsupported_dims
                ),
                Some(EntityRef::Phrase { id: phrase.id }),
            ));
        }

        // Frame times → ticks, for sampling the track's curves.
        let ticks: Vec<i64> = (0..frames)
            .map(|i| tempo.sec_to_tick(t0 + i as f64 * fs))
            .collect();
        let user = |param: &str| -> Option<Vec<f64>> {
            let c = &track.curves.get(param)?.user_delta;
            Some(
                ticks
                    .iter()
                    .map(|&t| c.value_in_span(t).unwrap_or(0.0))
                    .collect(),
            )
        };

        let f0_ref: Vec<f64> = match &pred.pitch {
            Some(p) => p.iter().map(|&c| f64::from(c)).collect(),
            None => base.clone(),
        };
        let f0_ref_hz: Vec<f32> = f0_ref.iter().map(|&c| cents_to_hz(c) as f32).collect();
        let f0_hz: Vec<f32> = match user(cantor_core::params::known::PITCH) {
            Some(d) => f0_ref
                .iter()
                .zip(&d)
                .map(|(&c, &u)| cents_to_hz(c + u) as f32)
                .collect(),
            None => f0_ref_hz.clone(),
        };

        let mut variance: BTreeMap<String, Vec<f32>> = BTreeMap::new();
        for &param in b.params() {
            if param == cantor_core::params::known::PITCH {
                continue;
            }
            let (lo, hi) = b.param_range(param);
            let predicted = pred.variance.get(param);
            let delta = user(param);
            let v: Vec<f32> = (0..frames)
                .map(|i| {
                    let p = predicted.map_or(b.param_default(param), |v| v[i]);
                    let d = delta.as_ref().map_or(0.0, |d| d[i] as f32);
                    (p + d).clamp(lo, hi)
                })
                .collect();
            variance.insert(param.to_owned(), v);
        }

        let mut mk = KeyBuilder::new("mel");
        mk.key("prediction", &pred_key)
            .u64("steps", u64::from(steps))
            .u64("acoustic_seed", phrase.seeds.acoustic);
        for (param, v) in &variance {
            mk.f32s(param, v);
        }
        let mel_key = mk.finish();
        let audio_key = KeyBuilder::new("audio")
            .key("mel", &mel_key)
            .f32s("f0", &f0_hz)
            .finish();

        let trace = |outcome| PhraseTrace {
            track: track.id,
            phrase: phrase.id,
            outcome,
            prediction_cached,
            audio_key: audio_key.hex(),
        };
        if let Some(audio) = self.cache.get_f32(Kind::Audio, &audio_key) {
            return Ok((audio, t0, trace(Outcome::CacheHit)));
        }
        let (mel, outcome) = match self.cache.get_f32(Kind::Mel, &mel_key) {
            Some(mel) => (mel, Outcome::VocoderFastPath),
            None => {
                let mel = b
                    .acoustic(&AcousticInput {
                        phones: &pred.phones,
                        f0_ref_hz: &f0_ref_hz,
                        variance: &variance,
                        steps,
                        seed: phrase.seeds.acoustic,
                    })
                    .map_err(backend_err)?;
                self.cache.put_f32(Kind::Mel, &mel_key, &mel)?;
                (mel, Outcome::Full)
            }
        };
        let audio = b.vocode(&mel, &f0_hz).map_err(backend_err)?;
        self.cache.put_f32(Kind::Audio, &audio_key, &audio)?;
        Ok((audio, t0, trace(outcome)))
    }
}

fn prediction_key(b: &dyn Backend, phrase: &Phrase, t0: f64, frames: usize, steps: u32) -> Key {
    let mut k = KeyBuilder::new("prediction");
    k.str("backend", b.id())
        .str("version", b.version())
        .u64("rng", u64::from(Rng::VERSION))
        .u64("t0", t0.to_bits())
        .u64("frames", frames as u64)
        .u64("steps", u64::from(steps))
        .u64("seed", phrase.seed)
        .u64("pitch_seed", phrase.seeds.pitch)
        .u64("variance_seed", phrase.seeds.variance);
    for n in &phrase.notes {
        k.u64("start", n.start_sec.to_bits())
            .u64("end", n.end_sec.to_bits())
            .u64("pitch", n.pitch_cents.to_bits())
            .str("lyric", &n.lyric)
            .str("lang", n.lang.as_ref().map_or("", |l| l.0.as_str()))
            .str(
                "phonemes",
                &serde_json::to_string(&n.phonemes).expect("phonemes serialize"),
            )
            .str(
                "expr",
                &serde_json::to_string(&n.expr).expect("expressions serialize"),
            );
    }
    k.finish()
}

/// Reports track settings and curves the backend cannot honor.
fn check_track_support(track: &Track, b: &dyn Backend, out: &mut Vec<Diagnostic>) {
    let target = |param: &ParamId| {
        Some(EntityRef::Curve {
            track: track.id,
            param: param.clone(),
        })
    };
    for (param, curve) in &track.curves {
        if !curve.user_delta.points.is_empty() && !b.params().contains(&param.as_str()) {
            out.push(Diagnostic::warning(
                "render.param_unsupported",
                format!(
                    "{} does not support `{param}`; the curve is ignored",
                    b.id()
                ),
                target(param),
            ));
        }
    }
    for part in &track.parts {
        for n in &part.notes {
            let e = &n.expr;
            let p = &e.portamento;
            if p.overshoot_cents != 0.0
                || p.preparation_cents != 0.0
                || e.attack.is_some()
                || e.release.is_some()
            {
                out.push(Diagnostic::warning(
                    "render.expr_unsupported",
                    "portamento overshoot/preparation and attack/release are not rendered yet (M4)",
                    Some(EntityRef::Note { id: n.id }),
                ));
            }
        }
    }
    let t = Some(EntityRef::Track { id: track.id });
    if !track.effects.is_empty() {
        out.push(Diagnostic::warning(
            "render.effects_unsupported",
            "the effects chain is not rendered yet (milestone M5); effects are ignored",
            t.clone(),
        ));
    }
    if track.doubling.layers > 1 {
        out.push(Diagnostic::warning(
            "render.doubling_unsupported",
            "voice doubling is not rendered yet (milestone M5); one layer is rendered",
            t.clone(),
        ));
    }
    if !track.voice_mix.is_empty() {
        out.push(Diagnostic::warning(
            "render.voice_mix_unsupported",
            format!("{} does not support voice mixing; it is ignored", b.id()),
            t,
        ));
    }
}
