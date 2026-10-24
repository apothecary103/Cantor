//! The document model.
//!
//! A [`Project`] is always kept in canonical form (see
//! [`Project::canonicalize`]) so that equal documents serialize to equal
//! bytes. Ops preserve canonical form; loaders and importers establish it.

use crate::diag::{Diagnostic, EntityRef};
use crate::ids::{NoteId, PartId, PhraseId, TakeId, Tick, TrackId};
use crate::params::ParamId;
use crate::time::{TempoMap, TimeSigMap};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A Cantor project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Project {
    /// Title and credits.
    #[serde(default)]
    pub meta: Meta,
    /// Tempo changes, sorted by tick; the first is at tick 0.
    pub tempo_map: Vec<TempoPoint>,
    /// Time-signature changes, sorted by bar; the first is at bar 0.
    pub time_sig_map: Vec<TimeSigPoint>,
    /// Key changes, sorted by tick. Drives scale highlighting and snapping.
    #[serde(default)]
    pub key_map: Vec<KeyPoint>,
    /// Tracks in display order.
    #[serde(default)]
    pub tracks: Vec<Track>,
    /// Diffusion noise seed per phrase.
    #[serde(default)]
    pub seeds: BTreeMap<PhraseId, u64>,
    /// Retakes per phrase. Takes are kept, not discarded.
    #[serde(default)]
    pub takes: BTreeMap<PhraseId, TakeLane>,
}

/// Title and credits.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Meta {
    /// Song title.
    pub title: String,
    /// Artist or author.
    pub artist: String,
    /// Free-form notes.
    pub comment: String,
}

/// A tempo change.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TempoPoint {
    /// Where the tempo takes effect.
    pub tick: Tick,
    /// Quarter notes per minute.
    pub bpm: f64,
}

/// A time-signature change, at the start of a bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimeSigPoint {
    /// 0-based bar where the signature takes effect.
    pub bar: u32,
    /// Beats per bar.
    pub numerator: u8,
    /// Beat unit, a power of two.
    pub denominator: u8,
}

/// A key change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeyPoint {
    /// Where the key takes effect.
    pub tick: Tick,
    /// Tonic pitch class, 0 = C … 11 = B.
    pub tonic: u8,
    /// Scale or mode.
    pub scale: Scale,
}

/// A scale or mode for key signatures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum Scale {
    Major,
    Minor,
    Dorian,
    Phrygian,
    Lydian,
    Mixolydian,
    Locrian,
    Chromatic,
}

/// A singing track.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Track {
    /// Unique id.
    pub id: TrackId,
    /// Display name.
    pub name: String,
    /// The voice that sings this track.
    pub singer: SingerRef,
    /// The synthesis backend.
    pub backend: BackendRef,
    /// Gain, pan, mute and solo.
    #[serde(default)]
    pub mix: Mix,
    /// Post-processing chain, in order.
    #[serde(default)]
    pub effects: Vec<EffectSlot>,
    /// Voice doubling.
    #[serde(default)]
    pub doubling: Doubling,
    /// Timbre blending with other singers, if the backend supports it.
    #[serde(default)]
    pub voice_mix: Vec<VoiceMixEntry>,
    /// Layered parameter curves, in absolute ticks.
    #[serde(default)]
    pub curves: BTreeMap<ParamId, LayeredCurve>,
    /// Parts, sorted by (start, id).
    #[serde(default)]
    pub parts: Vec<Part>,
}

/// Names a singer (voicebank).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SingerRef {
    /// Singer identifier; empty means none selected.
    pub id: String,
    /// Singer version, if pinned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Names a synthesis backend, such as `"diffsinger"` or `"utau"`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct BackendRef(pub String);

/// Track mixer settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Mix {
    /// Gain in dB.
    pub gain_db: f64,
    /// Pan, -1 (left) … 1 (right).
    pub pan: f64,
    /// Muted.
    pub mute: bool,
    /// Soloed.
    pub solo: bool,
}

impl Default for Mix {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
        }
    }
}

/// One effect in a track's chain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EffectSlot {
    /// Which effect.
    pub kind: EffectKind,
    /// Bypassed when false.
    pub enabled: bool,
    /// Effect parameters by name.
    #[serde(default)]
    pub params: BTreeMap<String, f64>,
}

/// Built-in effects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum EffectKind {
    Eq,
    Compressor,
    DeEsser,
    Reverb,
}

/// Voice doubling: extra layers with seeded micro-variations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Doubling {
    /// Number of voices, 1 (off) … 16.
    pub layers: u8,
    /// Stereo spread, 0 … 1.
    pub spacing: f64,
    /// Pitch variation between layers in cents.
    pub detune_cents: f64,
    /// Timing variation between layers in milliseconds.
    pub jitter_ms: f64,
    /// Seed for the variations.
    pub seed: u64,
}

impl Default for Doubling {
    fn default() -> Self {
        Self {
            layers: 1,
            spacing: 0.5,
            detune_cents: 8.0,
            jitter_ms: 12.0,
            seed: 0,
        }
    }
}

/// A singer blended into a track's timbre.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VoiceMixEntry {
    /// The blended singer.
    pub singer: SingerRef,
    /// Blend weight, 0 … 1.
    pub weight: f64,
}

/// A region of notes on a track. Note positions are relative to `start`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Part {
    /// Unique id.
    pub id: PartId,
    /// Display name.
    pub name: String,
    /// Absolute start tick.
    pub start: Tick,
    /// Length in ticks.
    pub length: Tick,
    /// Notes, sorted by (start, id).
    #[serde(default)]
    pub notes: Vec<Note>,
}

/// A sung note.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Note {
    /// Unique id.
    pub id: NoteId,
    /// Start tick relative to the part.
    pub start: Tick,
    /// Length in ticks, at least 1.
    pub length: Tick,
    /// Pitch in cents, MIDI note number × 100. Microtonal by default.
    pub pitch_cents: f64,
    /// Lyric text. `+` continues the previous syllable; a leading or
    /// trailing `-` joins syllables of one word.
    pub lyric: String,
    /// Language override; `None` uses the singer's default language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<LangTag>,
    /// Phoneme overrides; `None` lets the phonemizer decide.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phonemes: Option<Vec<PhonemeOverride>>,
    /// Vibrato, portamento, attack and release.
    #[serde(default)]
    pub expr: NoteExpr,
    /// Vocal mode weights.
    #[serde(default)]
    pub modes: VocalModes,
}

/// A BCP 47 language tag, such as `"ja"` or `"en-US"`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct LangTag(pub String);

/// A user-specified phoneme.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PhonemeOverride {
    /// Phoneme symbol.
    pub symbol: String,
    /// Symbol alphabet.
    #[serde(default)]
    pub alphabet: PhonemeAlphabet,
    /// Onset relative to the note onset in milliseconds; `None` = automatic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_ms: Option<f64>,
    /// Pinned length in milliseconds; `None` lets the duration model decide.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length_ms: Option<f64>,
}

/// Which symbol set a phoneme uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PhonemeAlphabet {
    /// The phonemizer's own symbols.
    #[default]
    Native,
    /// Universal X-SAMPA.
    XSampa,
}

/// Note expression.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct NoteExpr {
    /// Procedural vibrato, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vibrato: Option<Vibrato>,
    /// Transition into this note.
    pub portamento: Portamento,
    /// Onset shaping, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attack: Option<Envelope>,
    /// Release shaping, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release: Option<Envelope>,
}

/// Procedural vibrato: `v(t) = D · env(t) · sin(2π ∫ f(τ)dτ + φ)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Vibrato {
    /// Depth D in cents.
    pub depth_cents: f64,
    /// Rate in Hz.
    pub rate_hz: f64,
    /// Where vibrato starts, as a fraction of the note, 0 … 1.
    pub start: f64,
    /// Fade-in length as a fraction of the vibrato, 0 … 1.
    pub fade_in: f64,
    /// Fade-out length as a fraction of the vibrato, 0 … 1.
    pub fade_out: f64,
    /// Initial phase φ as a fraction of a cycle, 0 … 1.
    pub phase: f64,
    /// Random rate drift, 0 … 1 (1 ≈ ±5%).
    pub drift: f64,
}

/// Portamento into a note: a cosine-eased transition from the previous pitch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Portamento {
    /// Transition length in milliseconds; `None` = automatic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length_ms: Option<f64>,
    /// Fraction of the transition before the note boundary, 0 … 1.
    pub before: f64,
    /// Overshoot past the target pitch in cents.
    pub overshoot_cents: f64,
    /// Dip opposite to the transition direction before it starts, in cents.
    pub preparation_cents: f64,
}

impl Default for Portamento {
    fn default() -> Self {
        Self {
            length_ms: None,
            before: 0.6,
            overshoot_cents: 0.0,
            preparation_cents: 0.0,
        }
    }
}

/// Attack or release shaping.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    /// Length in milliseconds.
    pub length_ms: f64,
    /// Strength, 0 … 1.
    pub strength: f64,
}

/// Vocal mode weights per dimension (SynthV-style pitch / timbre /
/// pronunciation modes). Mode names come from the singer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct VocalModes {
    /// Pitch-mode weights by mode name.
    pub pitch: BTreeMap<String, f64>,
    /// Timbre-mode weights by mode name.
    pub timbre: BTreeMap<String, f64>,
    /// Pronunciation-mode weights by mode name.
    pub pronunciation: BTreeMap<String, f64>,
}

/// A layered parameter: `final = base(notes) + model_prediction + user_delta`.
/// Users and agents edit `user_delta`; the backend writes `model_prediction`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayeredCurve {
    /// Produced by the backend; retakeable.
    #[serde(default)]
    pub model_prediction: Curve,
    /// Edited by users and agents.
    #[serde(default)]
    pub user_delta: Curve,
}

impl LayeredCurve {
    /// True when neither layer has points.
    pub fn is_empty(&self) -> bool {
        self.model_prediction.points.is_empty() && self.user_delta.points.is_empty()
    }
}

/// A curve: control points with an interpolation mode.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Curve {
    /// `(tick, value)` points with strictly increasing ticks.
    pub points: Vec<(Tick, f64)>,
    /// How values between points are interpolated.
    #[serde(default)]
    pub interp: Interp,
}

/// Interpolation between curve points.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Interp {
    /// Hold each value until the next point.
    Step,
    /// Straight lines.
    Linear,
    /// Fritsch–Carlson monotone cubic; never overshoots.
    #[default]
    MonotoneHermite,
}

/// A half-open tick range `[start, end)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TickRange {
    /// First tick in the range.
    pub start: Tick,
    /// First tick after the range.
    pub end: Tick,
}

impl TickRange {
    /// Whether `t` lies in the range.
    pub fn contains(&self, t: Tick) -> bool {
        self.start <= t && t < self.end
    }
}

/// All takes of one phrase.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TakeLane {
    /// Every take, oldest first.
    pub takes: Vec<Take>,
    /// The take that plays; `None` = the original.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<TakeId>,
}

/// One retake of a phrase.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Take {
    /// Unique within the lane.
    pub id: TakeId,
    /// Seed used to generate the take.
    pub seed: u64,
    /// Which dimensions were re-generated.
    pub dims: Vec<RetakeDim>,
    /// Display label.
    #[serde(default)]
    pub label: String,
}

/// A dimension a retake can vary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum RetakeDim {
    Pitch,
    Timing,
    Timbre,
    Expression,
}

impl Default for Project {
    fn default() -> Self {
        Self::new()
    }
}

impl Project {
    /// An empty project at 120 BPM in 4/4.
    pub fn new() -> Self {
        Self {
            meta: Meta::default(),
            tempo_map: vec![TempoPoint {
                tick: 0,
                bpm: 120.0,
            }],
            time_sig_map: vec![TimeSigPoint {
                bar: 0,
                numerator: 4,
                denominator: 4,
            }],
            key_map: Vec::new(),
            tracks: Vec::new(),
            seeds: BTreeMap::new(),
            takes: BTreeMap::new(),
        }
    }

    /// Tick ↔ second conversion for this project's tempo map.
    pub fn tempo(&self) -> TempoMap {
        TempoMap::new(&self.tempo_map)
    }

    /// Bar ↔ tick conversion for this project's time-signature map.
    pub fn meter(&self) -> TimeSigMap {
        TimeSigMap::new(&self.time_sig_map)
    }

    /// Finds a track by id.
    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    /// Finds a part by id, with its track.
    pub fn part(&self, id: PartId) -> Option<(&Track, &Part)> {
        self.tracks
            .iter()
            .find_map(|t| t.parts.iter().find(|p| p.id == id).map(|p| (t, p)))
    }

    /// Finds a note by id, with its track and part.
    pub fn note(&self, id: NoteId) -> Option<(&Track, &Part, &Note)> {
        self.tracks.iter().find_map(|t| {
            t.parts
                .iter()
                .find_map(|p| p.notes.iter().find(|n| n.id == id).map(|n| (t, p, n)))
        })
    }

    /// Iterates all notes with their track and part.
    pub fn notes(&self) -> impl Iterator<Item = (&Track, &Part, &Note)> {
        self.tracks.iter().flat_map(|t| {
            t.parts
                .iter()
                .flat_map(move |p| p.notes.iter().map(move |n| (t, p, n)))
        })
    }

    /// An unused track id (one past the largest in use).
    pub fn next_track_id(&self) -> TrackId {
        TrackId(self.tracks.iter().map(|t| t.id.0 + 1).max().unwrap_or(1))
    }

    /// An unused part id.
    pub fn next_part_id(&self) -> PartId {
        let max = self
            .tracks
            .iter()
            .flat_map(|t| &t.parts)
            .map(|p| p.id.0 + 1);
        PartId(max.max().unwrap_or(1))
    }

    /// An unused note id.
    pub fn next_note_id(&self) -> NoteId {
        NoteId(self.notes().map(|(_, _, n)| n.id.0 + 1).max().unwrap_or(1))
    }

    /// Puts the project in canonical form: sorted maps, parts and notes in
    /// (start, id) order, curve points by tick, empty curves removed.
    /// Duplicate keys are left in place for [`Project::validate`] to report.
    pub fn canonicalize(&mut self) {
        self.tempo_map.sort_by_key(|p| p.tick);
        self.time_sig_map.sort_by_key(|p| p.bar);
        self.key_map.sort_by_key(|p| p.tick);
        for track in &mut self.tracks {
            track.parts.sort_by_key(|p| (p.start, p.id));
            for part in &mut track.parts {
                part.notes.sort_by_key(|n| (n.start, n.id));
            }
            track.curves.retain(|_, c| !c.is_empty());
            for curve in track.curves.values_mut() {
                curve.model_prediction.points.sort_by_key(|p| p.0);
                curve.user_delta.points.sort_by_key(|p| p.0);
            }
        }
    }

    /// Checks every invariant ops rely on. Errors make the project unusable;
    /// warnings are informational.
    pub fn validate(&self) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut err = |code: &str, msg: String, target: Option<EntityRef>| {
            out.push(Diagnostic::error(code, msg, target));
        };

        match self.tempo_map.first() {
            Some(p) if p.tick == 0 => {}
            _ => err(
                "tempo.missing_start",
                "the tempo map must have a point at tick 0".into(),
                Some(EntityRef::TempoMap),
            ),
        }
        if !strictly_increasing(self.tempo_map.iter().map(|p| p.tick)) {
            err(
                "tempo.unsorted",
                "tempo points must have strictly increasing ticks".into(),
                Some(EntityRef::TempoMap),
            );
        }
        for p in &self.tempo_map {
            if !valid_bpm(p.bpm) {
                err(
                    "tempo.invalid_bpm",
                    format!(
                        "tempo {} at tick {} is outside {MIN_BPM}..={MAX_BPM}",
                        p.bpm, p.tick
                    ),
                    Some(EntityRef::TempoMap),
                );
            }
        }
        match self.time_sig_map.first() {
            Some(p) if p.bar == 0 => {}
            _ => err(
                "time_sig.missing_start",
                "the time-signature map must have a point at bar 0".into(),
                Some(EntityRef::TimeSigMap),
            ),
        }
        if !strictly_increasing(self.time_sig_map.iter().map(|p| i64::from(p.bar))) {
            err(
                "time_sig.unsorted",
                "time signatures must have strictly increasing bars".into(),
                Some(EntityRef::TimeSigMap),
            );
        }
        for p in &self.time_sig_map {
            if !valid_time_sig(p.numerator, p.denominator) {
                err(
                    "time_sig.invalid",
                    format!(
                        "{}/{} at bar {} is not a valid time signature",
                        p.numerator, p.denominator, p.bar
                    ),
                    Some(EntityRef::TimeSigMap),
                );
            }
        }
        if !strictly_increasing(self.key_map.iter().map(|p| p.tick)) {
            err(
                "key.unsorted",
                "key changes must have strictly increasing ticks".into(),
                Some(EntityRef::KeyMap),
            );
        }
        for p in &self.key_map {
            if p.tonic > 11 || p.tick < 0 {
                err(
                    "key.invalid",
                    format!(
                        "key at tick {} has tonic {} (expected 0..=11) or a negative tick",
                        p.tick, p.tonic
                    ),
                    Some(EntityRef::KeyMap),
                );
            }
        }

        let mut track_ids = BTreeSet::new();
        let mut part_ids = BTreeSet::new();
        let mut note_ids = BTreeSet::new();
        for t in &self.tracks {
            if !track_ids.insert(t.id) {
                err(
                    "track.duplicate_id",
                    format!("{} is used twice", t.id),
                    Some(EntityRef::Track { id: t.id }),
                );
            }
            for issue in validate_track_settings(t) {
                err("track.invalid", issue, Some(EntityRef::Track { id: t.id }));
            }
            for (param, curve) in &t.curves {
                let target = Some(EntityRef::Curve {
                    track: t.id,
                    param: param.clone(),
                });
                if param.descriptor().is_none() {
                    err(
                        "curve.unknown_param",
                        format!("unknown parameter `{param}`"),
                        target.clone(),
                    );
                }
                for c in [&curve.model_prediction, &curve.user_delta] {
                    if !strictly_increasing(c.points.iter().map(|p| p.0)) {
                        err(
                            "curve.unsorted",
                            format!("`{param}` points must have strictly increasing ticks"),
                            target.clone(),
                        );
                    }
                    if c.points.iter().any(|p| !p.1.is_finite()) {
                        err(
                            "curve.non_finite",
                            format!("`{param}` has a non-finite value"),
                            target.clone(),
                        );
                    }
                }
            }
            if !sorted_by(t.parts.iter().map(|p| (p.start, p.id))) {
                err(
                    "part.unsorted",
                    format!("parts of {} are not in (start, id) order", t.id),
                    Some(EntityRef::Track { id: t.id }),
                );
            }
            for p in &t.parts {
                if !part_ids.insert(p.id) {
                    err(
                        "part.duplicate_id",
                        format!("{} is used twice", p.id),
                        Some(EntityRef::Part { id: p.id }),
                    );
                }
                if p.length < 1 || p.start < 0 {
                    err(
                        "part.invalid",
                        format!(
                            "{} has start {} and length {} (start ≥ 0, length ≥ 1)",
                            p.id, p.start, p.length
                        ),
                        Some(EntityRef::Part { id: p.id }),
                    );
                }
                if !sorted_by(p.notes.iter().map(|n| (n.start, n.id))) {
                    err(
                        "note.unsorted",
                        format!("notes of {} are not in (start, id) order", p.id),
                        Some(EntityRef::Part { id: p.id }),
                    );
                }
                for n in &p.notes {
                    if !note_ids.insert(n.id) {
                        err(
                            "note.duplicate_id",
                            format!("{} is used twice", n.id),
                            Some(EntityRef::Note { id: n.id }),
                        );
                    }
                    if let Some(issue) = validate_note(n) {
                        err("note.invalid", issue, Some(EntityRef::Note { id: n.id }));
                    }
                }
            }
        }
        for (phrase, lane) in &self.takes {
            if let Some(sel) = lane.selected
                && !lane.takes.iter().any(|t| t.id == sel)
            {
                err(
                    "take.not_found",
                    format!("{phrase} selects missing {sel}"),
                    Some(EntityRef::Phrase { id: *phrase }),
                );
            }
        }
        out
    }
}

/// Lowest accepted tempo.
pub const MIN_BPM: f64 = 1.0;
/// Highest accepted tempo.
pub const MAX_BPM: f64 = 1000.0;

pub(crate) fn valid_bpm(bpm: f64) -> bool {
    (MIN_BPM..=MAX_BPM).contains(&bpm)
}

pub(crate) fn valid_time_sig(numerator: u8, denominator: u8) -> bool {
    numerator >= 1 && denominator.is_power_of_two() && denominator <= 64
}

/// Returns a message if a note's own fields are invalid.
pub(crate) fn validate_note(n: &Note) -> Option<String> {
    if n.start < 0 {
        return Some(format!(
            "{} starts before its part (start {})",
            n.id, n.start
        ));
    }
    if n.length < 1 {
        return Some(format!("{} has length {}", n.id, n.length));
    }
    if !(0.0..=12_800.0).contains(&n.pitch_cents) {
        return Some(format!(
            "{} has pitch {} cents, outside 0..=12800",
            n.id, n.pitch_cents
        ));
    }
    if !expr_finite(&n.expr) {
        return Some(format!("{} has a non-finite expression value", n.id));
    }
    if let Some(ph) = &n.phonemes
        && ph.iter().any(|p| {
            p.symbol.is_empty()
                || !p.offset_ms.is_none_or(f64::is_finite)
                || !p.length_ms.is_none_or(|l| l.is_finite() && l >= 0.0)
        })
    {
        return Some(format!(
            "{} has an empty phoneme or invalid phoneme timing",
            n.id
        ));
    }
    let modes = [&n.modes.pitch, &n.modes.timbre, &n.modes.pronunciation];
    if modes.iter().any(|m| m.values().any(|w| !w.is_finite())) {
        return Some(format!("{} has a non-finite vocal mode weight", n.id));
    }
    None
}

fn expr_finite(e: &NoteExpr) -> bool {
    let vib = e.vibrato.as_ref().is_none_or(|v| {
        [
            v.depth_cents,
            v.rate_hz,
            v.start,
            v.fade_in,
            v.fade_out,
            v.phase,
            v.drift,
        ]
        .iter()
        .all(|x| x.is_finite())
    });
    let p = &e.portamento;
    let port = p.length_ms.is_none_or(f64::is_finite)
        && [p.before, p.overshoot_cents, p.preparation_cents]
            .iter()
            .all(|x| x.is_finite());
    let env = [&e.attack, &e.release].iter().all(|x| {
        x.as_ref()
            .is_none_or(|v| v.length_ms.is_finite() && v.strength.is_finite())
    });
    vib && port && env
}

/// Returns a message for each invalid track-level setting.
pub(crate) fn validate_track_settings(t: &Track) -> Vec<String> {
    let mut out = Vec::new();
    if !t.mix.gain_db.is_finite() || !(-1.0..=1.0).contains(&t.mix.pan) {
        out.push(format!("{} has invalid mix settings", t.id));
    }
    let d = &t.doubling;
    if !(1..=16).contains(&d.layers)
        || ![d.spacing, d.detune_cents, d.jitter_ms]
            .iter()
            .all(|x| x.is_finite())
    {
        out.push(format!(
            "{} has invalid doubling (layers must be 1..=16)",
            t.id
        ));
    }
    if t.voice_mix.iter().any(|v| !(0.0..=1.0).contains(&v.weight)) {
        out.push(format!("{} has a voice-mix weight outside 0..=1", t.id));
    }
    if t.effects
        .iter()
        .any(|e| e.params.values().any(|x| !x.is_finite()))
    {
        out.push(format!("{} has a non-finite effect parameter", t.id));
    }
    out
}

fn strictly_increasing(it: impl Iterator<Item = i64>) -> bool {
    let mut prev = None;
    for x in it {
        if prev.is_some_and(|p| p >= x) {
            return false;
        }
        prev = Some(x);
    }
    true
}

fn sorted_by<K: Ord>(it: impl Iterator<Item = K>) -> bool {
    let mut prev: Option<K> = None;
    for k in it {
        if prev.as_ref().is_some_and(|p| *p > k) {
            return false;
        }
        prev = Some(k);
    }
    true
}
