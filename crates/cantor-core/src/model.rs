//! The project model, file format v3 (docs/design.md, "Model").
//!
//! Every struct rejects unknown fields and fills omitted ones with
//! defaults, so clients can send minimal JSON: a note needs only `start`,
//! `length`, `key` and `lyric`.

use crate::ids::{NoteId, Tick, TrackId};
use crate::params::ParamId;
use crate::time::{TempoMap, TimeSigMap};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The current project file format.
pub const FORMAT: u32 = 3;

fn format_v3() -> u32 {
    FORMAT
}
fn pct100() -> f64 {
    100.0
}
fn yes() -> bool {
    true
}
fn is_false(b: &bool) -> bool {
    !b
}
fn is_zero(x: &f64) -> bool {
    *x == 0.0
}

/// A song: tempo, meter, sections, an optional reference recording and
/// singing tracks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Project {
    /// File format version; always 3.
    #[serde(default = "format_v3")]
    pub format: u32,
    /// Title, artist and comment.
    #[serde(default)]
    pub meta: Meta,
    /// Tempo changes, sorted by tick; the first is at tick 0.
    #[serde(default = "default_tempo")]
    pub tempo: Vec<TempoPoint>,
    /// Time-signature changes, sorted by bar; the first is at bar 0.
    #[serde(default = "default_time_sig")]
    pub time_sig: Vec<TimeSigPoint>,
    /// The song's key, for highlighting and snapping. `null` = none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<Key>,
    /// Named sections (verse, chorus, ...), sorted by start.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<Section>,
    /// The original song's vocal or mix, for A/B listening, pitch matching
    /// and render reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<Reference>,
    /// Singing tracks, in display order.
    #[serde(default)]
    pub tracks: Vec<Track>,
}

fn default_tempo() -> Vec<TempoPoint> {
    vec![TempoPoint::default()]
}
fn default_time_sig() -> Vec<TimeSigPoint> {
    vec![TimeSigPoint::default()]
}

impl Default for Project {
    fn default() -> Self {
        Self {
            format: FORMAT,
            meta: Meta::default(),
            tempo: default_tempo(),
            time_sig: default_time_sig(),
            key: None,
            sections: Vec::new(),
            reference: None,
            tracks: Vec::new(),
        }
    }
}

impl Project {
    /// The tempo map, for tick ↔ second conversion.
    pub fn tempo_map(&self) -> TempoMap {
        TempoMap::new(&self.tempo)
    }

    /// The time-signature map, for bar ↔ tick conversion.
    pub fn time_sig_map(&self) -> TimeSigMap {
        TimeSigMap::new(&self.time_sig)
    }

    /// Looks up a track.
    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    /// Looks up a track for mutation.
    pub fn track_mut(&mut self, id: TrackId) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    /// Finds a note in any track.
    pub fn find_note(&self, id: NoteId) -> Option<(&Track, &Note)> {
        self.tracks.iter().find_map(|t| t.note(id).map(|n| (t, n)))
    }

    /// The largest note id in use, or 0.
    pub fn max_note_id(&self) -> NoteId {
        self.tracks
            .iter()
            .flat_map(|t| &t.notes)
            .map(|n| n.id)
            .max()
            .unwrap_or_default()
    }

    /// The largest track id in use, or 0.
    pub fn max_track_id(&self) -> TrackId {
        self.tracks.iter().map(|t| t.id).max().unwrap_or_default()
    }

    /// The end of the last note of any track.
    pub fn end_tick(&self) -> Tick {
        self.tracks
            .iter()
            .filter_map(|t| t.notes.iter().map(Note::end).max())
            .max()
            .unwrap_or(0)
    }
}

/// Free-form project information.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Meta {
    /// Song title.
    pub title: String,
    /// Original artist.
    pub artist: String,
    /// Notes about the project.
    pub comment: String,
}

/// A tempo change.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TempoPoint {
    /// Where the tempo starts, in ticks (480 per quarter note).
    pub tick: Tick,
    /// Quarter notes per minute, 10..=1000.
    pub bpm: f64,
}

impl Default for TempoPoint {
    fn default() -> Self {
        Self {
            tick: 0,
            bpm: 120.0,
        }
    }
}

/// A time-signature change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimeSigPoint {
    /// The 0-based bar where the signature starts.
    pub bar: u32,
    /// Beats per bar, 1..=32.
    pub num: u8,
    /// Beat unit: 1, 2, 4, 8, 16 or 32.
    pub den: u8,
}

impl Default for TimeSigPoint {
    fn default() -> Self {
        Self {
            bar: 0,
            num: 4,
            den: 4,
        }
    }
}

/// A musical key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Key {
    /// Pitch class of the tonic, 0 (C) ..= 11 (B).
    pub tonic: u8,
    /// The scale.
    #[serde(default)]
    pub scale: Scale,
}

/// Scales for key highlighting and snapping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum Scale {
    #[default]
    Major,
    /// Natural minor.
    Minor,
    HarmonicMinor,
    Dorian,
    /// Major pentatonic.
    Pentatonic,
    MinorPentatonic,
}

impl Scale {
    /// Semitone offsets of the scale degrees from the tonic.
    pub fn steps(self) -> &'static [u8] {
        match self {
            Scale::Major => &[0, 2, 4, 5, 7, 9, 11],
            Scale::Minor => &[0, 2, 3, 5, 7, 8, 10],
            Scale::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            Scale::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            Scale::Pentatonic => &[0, 2, 4, 7, 9],
            Scale::MinorPentatonic => &[0, 3, 5, 7, 10],
        }
    }
}

impl Key {
    /// Whether MIDI note `key` is in this key.
    pub fn contains(&self, key: u8) -> bool {
        let pc = (u16::from(key) + 12 - u16::from(self.tonic % 12)) % 12;
        self.scale.steps().contains(&(pc as u8))
    }
}

/// A named span of the song.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Section {
    /// Name, such as "verse 1" or "chorus".
    pub name: String,
    /// First tick.
    pub start: Tick,
    /// Tick after the last; greater than `start`.
    pub end: Tick,
}

/// The original recording a cover is made from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    /// Audio file, relative to the project file (or absolute).
    pub path: String,
    /// Time in the audio file, in ms, that lines up with tick 0.
    #[serde(default)]
    pub offset_ms: f64,
    /// Playback gain in dB, -60..=12.
    #[serde(default)]
    pub gain_db: f64,
    /// Whether playback of the reference is muted.
    #[serde(default)]
    pub muted: bool,
}

/// How lyrics become oto.ini aliases.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Phonemizer {
    /// Pick from the voicebank's aliases (CV, VCV or CVVC).
    #[default]
    Auto,
    /// Japanese CV: one alias per kana ("あ", "か").
    JpCv,
    /// Japanese VCV: "a か" style aliases, previous vowel first.
    JpVcv,
    /// Japanese CVVC: CV aliases plus "a k" vowel-consonant links.
    JpCvvc,
    /// English ARPAsing (arpabet aliases).
    EnArpasing,
    /// The lyric is the alias, unchanged.
    Raw,
}

/// A singing track: one voicebank singing a monophonic line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Track {
    /// Unique id. Omit (or 0) in `add_track` to have one assigned.
    #[serde(default)]
    pub id: TrackId,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// Voicebank id, as listed by `list_voicebanks`. Empty = no singer.
    #[serde(default)]
    pub singer: String,
    /// How lyrics become aliases.
    #[serde(default)]
    pub phonemizer: Phonemizer,
    /// Default resampler flags for every note, such as "g-5B40".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub flags: String,
    /// Gain, pan, mute and solo.
    #[serde(default)]
    pub mix: Mix,
    /// Post-processing chain, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<EffectSlot>,
    /// Expression curves by parameter id (pitd, dyn, gen, bre, bri, tone).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub curves: BTreeMap<ParamId, Curve>,
    /// Notes sorted by (start, id), in absolute ticks; they never overlap.
    #[serde(default)]
    pub notes: Vec<Note>,
}

impl Default for Track {
    fn default() -> Self {
        Self {
            id: TrackId::UNASSIGNED,
            name: String::new(),
            singer: String::new(),
            phonemizer: Phonemizer::Auto,
            flags: String::new(),
            mix: Mix::default(),
            effects: Vec::new(),
            curves: BTreeMap::new(),
            notes: Vec::new(),
        }
    }
}

impl Track {
    /// The index of a note in `notes`.
    pub fn note_index(&self, id: NoteId) -> Option<usize> {
        self.notes.iter().position(|n| n.id == id)
    }

    /// Looks up a note.
    pub fn note(&self, id: NoteId) -> Option<&Note> {
        self.notes.iter().find(|n| n.id == id)
    }

    /// Looks up a note for mutation. Re-sort with
    /// [`Project::canonicalize`] after moving it.
    pub fn note_mut(&mut self, id: NoteId) -> Option<&mut Note> {
        self.notes.iter_mut().find(|n| n.id == id)
    }

    /// The notes before and after a note in time order, if any.
    pub fn neighbors(&self, id: NoteId) -> (Option<&Note>, Option<&Note>) {
        match self.note_index(id) {
            Some(i) => (
                i.checked_sub(1).map(|j| &self.notes[j]),
                self.notes.get(i + 1),
            ),
            None => (None, None),
        }
    }

    /// Notes that overlap `[start, end)`, in time order.
    pub fn notes_in(&self, start: Tick, end: Tick) -> impl Iterator<Item = &Note> {
        self.notes
            .iter()
            .filter(move |n| n.start < end && n.end() > start)
    }

    /// Every note with its start and end in seconds, in time order.
    pub fn timed_notes<'a>(&'a self, tempo: &'a TempoMap) -> impl Iterator<Item = TimedNote<'a>> {
        self.notes.iter().map(move |n| TimedNote {
            note: n,
            start_sec: tempo.tick_to_sec(n.start),
            end_sec: tempo.tick_to_sec(n.end()),
        })
    }
}

/// A note with its position in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimedNote<'a> {
    /// The note.
    pub note: &'a Note,
    /// Start in seconds.
    pub start_sec: f64,
    /// End in seconds.
    pub end_sec: f64,
}

impl TimedNote<'_> {
    /// Duration in seconds.
    pub fn duration_sec(&self) -> f64 {
        self.end_sec - self.start_sec
    }
}

/// Track gain, pan, mute and solo.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Mix {
    /// Gain in dB, -60..=12.
    pub gain_db: f64,
    /// Pan, -1 (left) ..= 1 (right).
    pub pan: f64,
    /// Silences the track.
    #[serde(skip_serializing_if = "is_false")]
    pub mute: bool,
    /// When any track is soloed, only soloed tracks play.
    #[serde(skip_serializing_if = "is_false")]
    pub solo: bool,
}

/// One effect in a track's chain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EffectSlot {
    /// Which effect.
    pub kind: EffectKind,
    /// Bypassed when false.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Parameters by id (see the effect registry); omitted ones take their
    /// defaults.
    #[serde(default)]
    pub params: BTreeMap<String, f64>,
}

/// Built-in effects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    /// Parametric equalizer.
    Eq,
    /// Dynamic range compressor.
    Compressor,
    /// Sibilance reducer.
    DeEsser,
    /// Room reverb.
    Reverb,
    /// Feedback echo.
    Delay,
}

/// A note: a lyric sung at a pitch for a span of ticks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Note {
    /// Unique id within the project. Omit (or 0) in `add_notes` to have one
    /// assigned.
    #[serde(default)]
    pub id: NoteId,
    /// Start in absolute ticks (480 per quarter note), >= 0.
    pub start: Tick,
    /// Length in ticks, > 0.
    pub length: Tick,
    /// MIDI note number, 0..=127 (60 = C4).
    pub key: u8,
    /// What is sung: "あ", "ka", "+" (extend the previous note), "-" (rest
    /// with breath release), "br" (breath).
    pub lyric: String,
    /// Forces this exact oto.ini alias; omitted = the phonemizer decides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// Consonant velocity in %, 0..=200; above 100 shortens the consonant.
    #[serde(default = "pct100")]
    pub vel: f64,
    /// Intensity (volume) in %, 0..=200.
    #[serde(default = "pct100")]
    pub vol: f64,
    /// Modulation: % of the sample's own pitch wobble kept, 0..=100.
    #[serde(rename = "mod", default, skip_serializing_if = "is_zero")]
    pub mod_: f64,
    /// Resampler flags appended to the track's flags.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub flags: String,
    /// Overrides of the oto.ini timing.
    #[serde(default, skip_serializing_if = "Timing::is_default")]
    pub timing: Timing,
    /// Portamento, scoop, fall, vibrato and extra bend points.
    #[serde(default)]
    pub pitch: NotePitch,
}

impl Note {
    /// A note with default expression.
    pub fn new(start: Tick, length: Tick, key: u8, lyric: impl Into<String>) -> Self {
        Self {
            id: NoteId::UNASSIGNED,
            start,
            length,
            key,
            lyric: lyric.into(),
            alias: None,
            vel: 100.0,
            vol: 100.0,
            mod_: 0.0,
            flags: String::new(),
            timing: Timing::default(),
            pitch: NotePitch::default(),
        }
    }

    /// The tick after the note's last.
    pub fn end(&self) -> Tick {
        self.start + self.length
    }

    /// Whether the lyric extends the previous note ("+", "+~", "+*", ...).
    pub fn is_extension(&self) -> bool {
        self.lyric.starts_with('+')
    }

    /// Whether the note is a rest or breath ("-", "r", "R", "br", "息",
    /// "breath" or an empty lyric): it ends a phrase.
    pub fn is_rest_or_breath(&self) -> bool {
        matches!(
            self.lyric.trim(),
            "" | "-" | "r" | "R" | "rest" | "br" | "息" | "breath"
        )
    }
}

/// Per-note overrides of oto.ini timing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Timing {
    /// Pre-utterance in ms (how early the sample starts before the note);
    /// omitted = the alias's oto value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preutter_ms: Option<f64>,
    /// Overlap with the previous note in ms; omitted = the oto value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overlap_ms: Option<f64>,
    /// Extra start offset into the sample in ms.
    pub offset_ms: f64,
}

impl Timing {
    /// Whether nothing is overridden.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// A note's pitch expression. The final pitch at time t is
/// `key·100 + portamento + scoop + fall + vibrato + points + pitd(t)` cents.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct NotePitch {
    /// The glide from the previous note's pitch into this one.
    pub portamento: Portamento,
    /// Start below (cents < 0) or above the pitch and bend in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scoop: Option<Bend>,
    /// Bend away at the note's end (cents < 0 drops).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fall: Option<Bend>,
    /// Vibrato near the note's end.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vibrato: Option<Vibrato>,
    /// Extra bend points relative to the note start, interpolated smoothly.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<PitchPoint>,
}

/// The transition from the previous note's pitch into this note.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Portamento {
    /// Glide duration in ms, 0..=1000; 0 = hard step.
    pub length_ms: f64,
    /// Where the glide starts relative to the note start, in ms,
    /// -1000..=1000; negative starts before the note.
    pub start_ms: f64,
    /// How far past the target the glide goes before settling, in cents,
    /// 0..=200.
    pub overshoot_cents: f64,
    /// Dip opposite to the jump before the glide, in cents, 0..=200.
    pub preparation_cents: f64,
    /// Easing of the glide.
    pub shape: Shape,
}

impl Default for Portamento {
    fn default() -> Self {
        Self {
            length_ms: 80.0,
            start_ms: -40.0,
            overshoot_cents: 0.0,
            preparation_cents: 0.0,
            shape: Shape::S,
        }
    }
}

/// Easing of a glide.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    /// Ease in and out.
    #[default]
    S,
    /// Constant speed.
    Linear,
    /// Slow start, fast end.
    In,
    /// Fast start, slow end.
    Out,
}

/// A bend at a note's start (scoop) or end (fall).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Bend {
    /// Pitch offset at the far end of the bend, in cents, -1200..=1200.
    pub cents: f64,
    /// Duration in ms, 0..=2000.
    pub length_ms: f64,
}

/// Vibrato over the end portion of a note.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Vibrato {
    /// % of the note, from its end, that vibrates, 0..=100.
    pub length: f64,
    /// Period in ms (1000 / rate in Hz), 50..=1000; 150..220 is typical.
    pub period_ms: f64,
    /// Depth in cents (half peak-to-peak), 0..=200.
    pub depth_cents: f64,
    /// Fade-in as % of the vibrato length, 0..=100.
    pub fade_in: f64,
    /// Fade-out as % of the vibrato length, 0..=100.
    pub fade_out: f64,
    /// Starting phase as a fraction of a cycle, 0..=1.
    pub phase: f64,
    /// Center offset as % of depth, -100..=100.
    pub shift: f64,
    /// Slow random wander of rate and depth, 0..=1, seeded by the note id.
    pub drift: f64,
}

impl Default for Vibrato {
    fn default() -> Self {
        Self {
            length: 65.0,
            period_ms: 175.0,
            depth_cents: 20.0,
            fade_in: 30.0,
            fade_out: 20.0,
            phase: 0.0,
            shift: 0.0,
            drift: 0.0,
        }
    }
}

/// An extra pitch-bend point within a note.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PitchPoint {
    /// Time relative to the note start, in ms.
    pub ms: f64,
    /// Pitch offset in cents, -1200..=1200.
    pub cents: f64,
}

/// A track expression curve: points interpolated over ticks.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Curve {
    /// `[tick, value]` pairs sorted by tick with unique ticks. Before the
    /// first and after the last point the nearest value holds.
    #[serde(default)]
    pub points: Vec<(Tick, f64)>,
    /// How values between points are interpolated.
    #[serde(default)]
    pub interp: Interp,
}

/// Interpolation between curve points.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Interp {
    /// Straight lines.
    #[default]
    Linear,
    /// Hold each value until the next point.
    Step,
    /// Monotone cubic: smooth and never overshoots the points.
    Smooth,
}
