//! [`Op`]: every edit to a project. Clients never mutate a [`Project`]
//! directly; they send ops, and each applied op returns its inverse.

use crate::ids::{NoteId, Tick, TrackId};
use crate::model::*;
use crate::params::ParamId;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

/// An edit. JSON: `{"op": "<name>", ...fields}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
// Ops are few and short-lived; boxing the patches would only cost ergonomics.
#[allow(clippy::large_enum_variant)]
pub enum Op {
    /// Several ops applied atomically as one undo step: if any fails,
    /// none is applied.
    Batch {
        /// Name in the undo history.
        #[serde(default)]
        label: String,
        /// The ops, applied in order.
        ops: Vec<Op>,
    },
    /// Replaces title, artist and comment.
    SetMeta {
        /// The new metadata.
        meta: Meta,
    },
    /// Sets the tempo from `tick` on, adding or replacing a tempo point.
    SetTempo {
        /// Tick of the tempo point, >= 0.
        tick: Tick,
        /// Quarter notes per minute, 10..=1000.
        bpm: f64,
    },
    /// Removes the tempo point at `tick` (not the one at 0).
    RemoveTempo {
        /// Tick of an existing tempo point.
        tick: Tick,
    },
    /// Sets the time signature from `bar` on.
    SetTimeSig {
        /// 0-based bar.
        bar: u32,
        /// Beats per bar, 1..=32.
        num: u8,
        /// Beat unit: 1, 2, 4, 8, 16 or 32.
        den: u8,
    },
    /// Removes the time-signature change at `bar` (not bar 0).
    RemoveTimeSig {
        /// Bar of an existing change.
        bar: u32,
    },
    /// Sets or clears (`null`) the song's key.
    SetKey {
        /// The key.
        #[serde(default)]
        key: Option<Key>,
    },
    /// Replaces all sections.
    SetSections {
        /// The sections; each needs `start < end`.
        sections: Vec<Section>,
    },
    /// Sets or clears (`null`) the reference recording.
    SetReference {
        /// The reference.
        #[serde(default)]
        reference: Option<Reference>,
    },
    /// Adds a track. Its id and its notes' ids are assigned when 0/omitted.
    AddTrack {
        /// The track, possibly with notes, curves and effects.
        track: Track,
        /// Position in the track list; omitted = last.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
    },
    /// Removes a track and everything on it.
    RemoveTrack {
        /// The track.
        track: TrackId,
    },
    /// Changes a track's settings (not its notes, curves or effects).
    UpdateTrack {
        /// The track.
        track: TrackId,
        /// Fields to change; omitted fields stay.
        patch: TrackPatch,
    },
    /// Adds notes. Ids that are 0/omitted are assigned.
    AddNotes {
        /// The track.
        track: TrackId,
        /// The notes, in absolute ticks.
        notes: Vec<Note>,
    },
    /// Removes notes.
    RemoveNotes {
        /// The track.
        track: TrackId,
        /// The notes.
        ids: Vec<NoteId>,
    },
    /// Sets the same fields on several notes.
    UpdateNotes {
        /// The track.
        track: TrackId,
        /// The notes.
        ids: Vec<NoteId>,
        /// Fields to change; omitted fields stay.
        patch: NotePatch,
    },
    /// Moves notes in time and/or pitch.
    MoveNotes {
        /// The track.
        track: TrackId,
        /// The notes.
        ids: Vec<NoteId>,
        /// Ticks to move by; results must be >= 0.
        #[serde(default)]
        dticks: Tick,
        /// Semitones to transpose by; results must be in 0..=127.
        #[serde(default)]
        dkeys: i32,
    },
    /// Lengthens (dlength > 0) or shortens notes.
    ResizeNotes {
        /// The track.
        track: TrackId,
        /// The notes.
        ids: Vec<NoteId>,
        /// Ticks to add to each length; results must be > 0.
        dlength: Tick,
        /// Move the start edge (start -= dlength, end fixed) instead of the
        /// end edge.
        #[serde(default)]
        from_start: bool,
    },
    /// Splits a note at `at`. The left part keeps the id, lyric, scoop and
    /// portamento; the right part gets a new id, lyric "+", the vibrato
    /// and the fall.
    SplitNote {
        /// The track.
        track: TrackId,
        /// The note.
        id: NoteId,
        /// Absolute tick strictly inside the note.
        at: Tick,
    },
    /// Merges notes into the earliest one, which then spans until the
    /// latest end; the others are removed.
    MergeNotes {
        /// The track.
        track: TrackId,
        /// At least two notes.
        ids: Vec<NoteId>,
    },
    /// Sets lyrics: the i-th lyric goes to the i-th id.
    SetLyrics {
        /// The track.
        track: TrackId,
        /// The notes.
        ids: Vec<NoteId>,
        /// One lyric per id: a list, or one string split at whitespace.
        lyrics: Lyrics,
    },
    /// Replaces a curve's points within `range` (inclusive) by `points`.
    SetCurve {
        /// The track.
        track: TrackId,
        /// pitd, dyn, gen, bre, bri or tone.
        param: ParamId,
        /// `[first, last]` tick, inclusive. All points must lie inside.
        range: (Tick, Tick),
        /// `[tick, value]` pairs; values within the parameter's range.
        points: Vec<(Tick, f64)>,
        /// Also sets the curve's interpolation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interp: Option<Interp>,
    },
    /// Removes a curve's points within `range` (inclusive), or all points.
    ClearCurve {
        /// The track.
        track: TrackId,
        /// pitd, dyn, gen, bre, bri or tone.
        param: ParamId,
        /// `[first, last]` tick, inclusive; omitted = everything.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        range: Option<(Tick, Tick)>,
    },
    /// Replaces a track's effect chain.
    SetEffects {
        /// The track.
        track: TrackId,
        /// The chain, in order.
        effects: Vec<EffectSlot>,
    },
}

impl Op {
    /// A short name for the undo history.
    pub fn label(&self) -> String {
        let s = match self {
            Op::Batch { label, ops } if label.is_empty() => {
                return match ops.as_slice() {
                    [one] => one.label(),
                    _ => format!("{} edits", ops.len()),
                };
            }
            Op::Batch { label, .. } => return label.clone(),
            Op::SetMeta { .. } => "Edit project info",
            Op::SetTempo { .. } => "Set tempo",
            Op::RemoveTempo { .. } => "Remove tempo",
            Op::SetTimeSig { .. } => "Set time signature",
            Op::RemoveTimeSig { .. } => "Remove time signature",
            Op::SetKey { .. } => "Set key",
            Op::SetSections { .. } => "Edit sections",
            Op::SetReference { .. } => "Set reference",
            Op::AddTrack { .. } => "Add track",
            Op::RemoveTrack { .. } => "Remove track",
            Op::UpdateTrack { .. } => "Edit track",
            Op::AddNotes { .. } => "Add notes",
            Op::RemoveNotes { .. } => "Remove notes",
            Op::UpdateNotes { .. } => "Edit notes",
            Op::MoveNotes { .. } => "Move notes",
            Op::ResizeNotes { .. } => "Resize notes",
            Op::SplitNote { .. } => "Split note",
            Op::MergeNotes { .. } => "Merge notes",
            Op::SetLyrics { .. } => "Set lyrics",
            Op::SetCurve { .. } => "Draw curve",
            Op::ClearCurve { .. } => "Clear curve",
            Op::SetEffects { .. } => "Edit effects",
        };
        s.into()
    }

    /// A batch with a label.
    pub fn batch(label: impl Into<String>, ops: Vec<Op>) -> Self {
        Op::Batch {
            label: label.into(),
            ops,
        }
    }

    /// The ops that turn `from` into `to` (track contents are replaced
    /// wholesale when they differ). Used to restore checkpoints and to make
    /// imports undoable.
    pub fn transition(from: &Project, to: &Project) -> Vec<Op> {
        let mut ops = Vec::new();
        if from.meta != to.meta {
            ops.push(Op::SetMeta {
                meta: to.meta.clone(),
            });
        }
        for p in &from.tempo {
            if p.tick != 0 && !to.tempo.iter().any(|q| q.tick == p.tick) {
                ops.push(Op::RemoveTempo { tick: p.tick });
            }
        }
        for p in &to.tempo {
            if !from.tempo.contains(p) {
                ops.push(Op::SetTempo {
                    tick: p.tick,
                    bpm: p.bpm,
                });
            }
        }
        for s in &from.time_sig {
            if s.bar != 0 && !to.time_sig.iter().any(|t| t.bar == s.bar) {
                ops.push(Op::RemoveTimeSig { bar: s.bar });
            }
        }
        for s in &to.time_sig {
            if !from.time_sig.contains(s) {
                ops.push(Op::SetTimeSig {
                    bar: s.bar,
                    num: s.num,
                    den: s.den,
                });
            }
        }
        if from.key != to.key {
            ops.push(Op::SetKey { key: to.key });
        }
        if from.sections != to.sections {
            ops.push(Op::SetSections {
                sections: to.sections.clone(),
            });
        }
        if from.reference != to.reference {
            ops.push(Op::SetReference {
                reference: to.reference.clone(),
            });
        }
        if from.tracks != to.tracks {
            ops.extend(from.tracks.iter().map(|t| Op::RemoveTrack { track: t.id }));
            ops.extend(to.tracks.iter().map(|t| Op::AddTrack {
                track: t.clone(),
                index: None,
            }));
        }
        ops
    }
}

/// Lyrics for [`Op::SetLyrics`]: a list, or one string split at whitespace.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Lyrics {
    /// One lyric per note.
    List(Vec<String>),
    /// Lyrics separated by whitespace, such as "ka ra o ke".
    Text(String),
}

impl Lyrics {
    /// The lyrics as a list.
    pub fn to_vec(&self) -> Vec<String> {
        match self {
            Lyrics::List(v) => v.clone(),
            Lyrics::Text(s) => s.split_whitespace().map(String::from).collect(),
        }
    }
}

/// Deserializes a present field (even `null`) as `Some`, so a patch can
/// tell "clear" (`null`) from "leave alone" (omitted).
fn present<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    d: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

/// Assigns each `Some` field of a patch to the target.
macro_rules! patch {
    ($target:expr, $patch:expr; $($field:ident),* $(,)?) => {
        $(if let Some(v) = &$patch.$field { $target.$field = v.clone(); })*
    };
}

/// Changes to a track's settings; omitted fields stay as they are.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct TrackPatch {
    /// Display name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Voicebank id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub singer: Option<String>,
    /// How lyrics become aliases.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phonemizer: Option<Phonemizer>,
    /// Default resampler flags.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flags: Option<String>,
    /// Gain, pan, mute and solo.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mix: Option<MixPatch>,
}

/// Changes to [`Mix`]; omitted fields stay.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct MixPatch {
    /// Gain in dB, -60..=12.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gain_db: Option<f64>,
    /// Pan, -1..=1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pan: Option<f64>,
    /// Mute.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mute: Option<bool>,
    /// Solo.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solo: Option<bool>,
}

impl TrackPatch {
    /// A patch that sets every field to the track's current value.
    pub fn from_track(t: &Track) -> Self {
        Self {
            name: Some(t.name.clone()),
            singer: Some(t.singer.clone()),
            phonemizer: Some(t.phonemizer),
            flags: Some(t.flags.clone()),
            mix: Some(MixPatch {
                gain_db: Some(t.mix.gain_db),
                pan: Some(t.mix.pan),
                mute: Some(t.mix.mute),
                solo: Some(t.mix.solo),
            }),
        }
    }

    /// Applies the patch.
    pub fn apply_to(&self, t: &mut Track) {
        patch!(t, self; name, singer, phonemizer, flags);
        if let Some(m) = &self.mix {
            patch!(t.mix, m; gain_db, pan, mute, solo);
        }
    }
}

/// Changes to notes; omitted fields stay as they are. Nested objects merge
/// (e.g. `{"pitch": {"vibrato": {"depth_cents": 30}}}` changes only the
/// depth, starting from the default vibrato if the note had none);
/// `null` clears optional fields (alias, scoop, fall, vibrato,
/// timing.preutter_ms, timing.overlap_ms).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct NotePatch {
    /// Start in absolute ticks, >= 0.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<Tick>,
    /// Length in ticks, > 0.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length: Option<Tick>,
    /// MIDI note number, 0..=127.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<u8>,
    /// Lyric.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lyric: Option<String>,
    /// Forced oto alias; `null` = let the phonemizer decide.
    #[serde(deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub alias: Option<Option<String>>,
    /// Consonant velocity %, 0..=200.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vel: Option<f64>,
    /// Intensity %, 0..=200.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vol: Option<f64>,
    /// Modulation %, 0..=100.
    #[serde(rename = "mod", skip_serializing_if = "Option::is_none")]
    pub mod_: Option<f64>,
    /// Per-note resampler flags.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flags: Option<String>,
    /// Timing overrides.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing: Option<TimingPatch>,
    /// Pitch expression.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch: Option<PitchPatch>,
}

/// Changes to [`Timing`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct TimingPatch {
    /// Pre-utterance ms; `null` = use oto.ini.
    #[serde(deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub preutter_ms: Option<Option<f64>>,
    /// Overlap ms; `null` = use oto.ini.
    #[serde(deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub overlap_ms: Option<Option<f64>>,
    /// Start offset ms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset_ms: Option<f64>,
}

/// Changes to [`NotePitch`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct PitchPatch {
    /// Portamento fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub portamento: Option<PortamentoPatch>,
    /// Scoop; `null` removes it.
    #[serde(deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub scoop: Option<Option<Bend>>,
    /// Fall; `null` removes it.
    #[serde(deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub fall: Option<Option<Bend>>,
    /// Vibrato fields; `null` removes the vibrato.
    #[serde(deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub vibrato: Option<Option<VibratoPatch>>,
    /// Replaces the extra bend points.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub points: Option<Vec<PitchPoint>>,
}

/// Changes to [`Portamento`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
#[allow(missing_docs)]
pub struct PortamentoPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overshoot_cents: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preparation_cents: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shape: Option<Shape>,
}

/// Changes to [`Vibrato`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
#[allow(missing_docs)]
pub struct VibratoPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth_cents: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fade_in: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fade_out: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shift: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drift: Option<f64>,
}

impl VibratoPatch {
    /// A patch that sets every field.
    pub fn from_vibrato(v: &Vibrato) -> Self {
        Self {
            length: Some(v.length),
            period_ms: Some(v.period_ms),
            depth_cents: Some(v.depth_cents),
            fade_in: Some(v.fade_in),
            fade_out: Some(v.fade_out),
            phase: Some(v.phase),
            shift: Some(v.shift),
            drift: Some(v.drift),
        }
    }
}

impl NotePatch {
    /// A patch that sets every field to the note's current value; applying
    /// it to any note makes that note equal to `n` (except for its id).
    pub fn from_note(n: &Note) -> Self {
        let p = &n.pitch;
        Self {
            start: Some(n.start),
            length: Some(n.length),
            key: Some(n.key),
            lyric: Some(n.lyric.clone()),
            alias: Some(n.alias.clone()),
            vel: Some(n.vel),
            vol: Some(n.vol),
            mod_: Some(n.mod_),
            flags: Some(n.flags.clone()),
            timing: Some(TimingPatch {
                preutter_ms: Some(n.timing.preutter_ms),
                overlap_ms: Some(n.timing.overlap_ms),
                offset_ms: Some(n.timing.offset_ms),
            }),
            pitch: Some(PitchPatch {
                portamento: Some(PortamentoPatch {
                    length_ms: Some(p.portamento.length_ms),
                    start_ms: Some(p.portamento.start_ms),
                    overshoot_cents: Some(p.portamento.overshoot_cents),
                    preparation_cents: Some(p.portamento.preparation_cents),
                    shape: Some(p.portamento.shape),
                }),
                scoop: Some(p.scoop),
                fall: Some(p.fall),
                vibrato: Some(p.vibrato.as_ref().map(VibratoPatch::from_vibrato)),
                points: Some(p.points.clone()),
            }),
        }
    }

    /// Applies the patch to a note (without validating the result).
    pub fn apply_to(&self, n: &mut Note) {
        patch!(n, self; start, length, key, lyric, alias, vel, vol, mod_, flags);
        if let Some(t) = &self.timing {
            patch!(n.timing, t; preutter_ms, overlap_ms, offset_ms);
        }
        let Some(pp) = &self.pitch else { return };
        let pitch = &mut n.pitch;
        if let Some(port) = &pp.portamento {
            patch!(pitch.portamento, port; length_ms, start_ms, overshoot_cents, preparation_cents, shape);
        }
        patch!(pitch, pp; scoop, fall, points);
        match &pp.vibrato {
            None => {}
            Some(None) => pitch.vibrato = None,
            Some(Some(vp)) => {
                let v = pitch.vibrato.get_or_insert_with(Vibrato::default);
                patch!(v, vp; length, period_ms, depth_cents, fade_in, fade_out, phase, shift, drift);
            }
        }
    }
}
