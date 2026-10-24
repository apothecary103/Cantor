//! Ops: the only way to change a project.
//!
//! Every op is serializable, validated before it touches the document, and
//! applied atomically. Applying an op yields its inverse, which the undo
//! tree stores; inverses restore the previous state exactly.

use crate::ids::{NoteId, PartId, PhraseId, TakeId, Tick, TrackId};
use crate::model::{
    BackendRef, Doubling, EffectSlot, Envelope, Interp, LayeredCurve, Meta, Mix, Note, Part,
    PhonemeOverride, Portamento, Scale, SingerRef, TickRange, Track, Vibrato, VocalModes,
    VoiceMixEntry,
};
use crate::params::ParamId;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

/// An edit to a project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Applies several ops atomically, as one undo step.
    Batch {
        /// Name shown in the history.
        label: String,
        /// Ops in order.
        ops: Vec<Op>,
    },

    /// Changes title and credits.
    SetMeta {
        /// The new metadata.
        meta: Meta,
    },
    /// Adds or replaces the tempo at a tick.
    SetTempo {
        /// Where the tempo takes effect, ≥ 0.
        tick: Tick,
        /// Quarter notes per minute.
        bpm: f64,
    },
    /// Removes the tempo change at a tick. The tempo at tick 0 cannot be
    /// removed.
    RemoveTempo {
        /// Tick of the tempo change.
        tick: Tick,
    },
    /// Adds or replaces the time signature at a bar.
    SetTimeSig {
        /// 0-based bar.
        bar: u32,
        /// Beats per bar.
        numerator: u8,
        /// Beat unit, a power of two.
        denominator: u8,
    },
    /// Removes the time signature at a bar (not bar 0).
    RemoveTimeSig {
        /// 0-based bar.
        bar: u32,
    },
    /// Adds or replaces the key at a tick.
    SetKey {
        /// Where the key takes effect, ≥ 0.
        tick: Tick,
        /// Tonic pitch class, 0 = C … 11 = B.
        tonic: u8,
        /// Scale or mode.
        scale: Scale,
    },
    /// Removes the key change at a tick.
    RemoveKey {
        /// Tick of the key change.
        tick: Tick,
    },
    /// Sets or clears a phrase's diffusion noise seed.
    SetSeed {
        /// The phrase.
        phrase: PhraseId,
        /// The seed; `None` removes it.
        seed: Option<u64>,
    },
    /// Chooses which take of a phrase plays.
    SelectTake {
        /// The phrase.
        phrase: PhraseId,
        /// The take; `None` selects the original.
        take: Option<TakeId>,
    },

    /// Inserts a track (with any parts, notes and curves it carries).
    AddTrack {
        /// Position in the track list; `None` appends.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
        /// The track. All ids inside must be unused.
        track: Track,
    },
    /// Removes a track and everything on it.
    RemoveTrack {
        /// The track.
        id: TrackId,
    },
    /// Moves a track to a new position in the track list.
    MoveTrack {
        /// The track.
        id: TrackId,
        /// New position.
        index: usize,
    },
    /// Changes track settings. Fields left out are unchanged.
    SetTrack {
        /// The track.
        id: TrackId,
        /// The changes.
        patch: TrackPatch,
    },

    /// Adds a part (with any notes it carries) to a track.
    AddPart {
        /// The track.
        track: TrackId,
        /// The part. All ids inside must be unused.
        part: Part,
    },
    /// Removes a part and its notes.
    RemovePart {
        /// The part.
        id: PartId,
    },
    /// Moves a part in time. Its notes move with it.
    MovePart {
        /// The part.
        id: PartId,
        /// Tick offset.
        dt: i64,
        /// Also move the track's curve points under the part.
        carry_curves: bool,
    },
    /// Renames or resizes a part.
    SetPart {
        /// The part.
        id: PartId,
        /// New name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// New length in ticks.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        length: Option<Tick>,
    },

    /// Adds a note to a part.
    AddNote {
        /// The part.
        part: PartId,
        /// The note; its id must be unused.
        note: Note,
    },
    /// Removes notes.
    RemoveNotes {
        /// The notes.
        ids: Vec<NoteId>,
    },
    /// Moves notes in time and pitch.
    MoveNotes {
        /// The notes.
        ids: Vec<NoteId>,
        /// Tick offset.
        dt: i64,
        /// Pitch offset in cents.
        dcents: f64,
        /// Also move the track's curve points under the notes (default).
        carry_curves: bool,
    },
    /// Changes a note's length.
    ResizeNote {
        /// The note.
        id: NoteId,
        /// New length in ticks, ≥ 1.
        length: Tick,
    },
    /// Changes a note's lyric.
    SetLyric {
        /// The note.
        id: NoteId,
        /// New lyric.
        lyric: String,
    },
    /// Sets or clears a note's phoneme overrides.
    SetPhonemes {
        /// The note.
        id: NoteId,
        /// Overrides; `None` returns control to the phonemizer.
        phonemes: Option<Vec<PhonemeOverride>>,
    },
    /// Changes a note's expression. Fields left out are unchanged.
    SetNoteExpr {
        /// The note.
        id: NoteId,
        /// The changes.
        expr: NoteExprPatch,
    },
    /// Replaces a note's vocal mode weights.
    SetVocalModes {
        /// The note.
        id: NoteId,
        /// New weights.
        modes: VocalModes,
    },
    /// Replaces existing notes wholesale, matched by id. Each note stays in
    /// its part. Undo uses this to restore notes exactly.
    ReplaceNotes {
        /// The new note contents.
        notes: Vec<Note>,
    },

    /// Replaces the user layer of a curve within a range: points in
    /// `[range.start, range.end)` are removed, then `points` are inserted.
    SetCurve {
        /// The track.
        track: TrackId,
        /// The parameter.
        param: ParamId,
        /// Range to replace.
        range: TickRange,
        /// New points, strictly increasing ticks, all inside `range`.
        points: Vec<(Tick, f64)>,
        /// Interpolation for the whole user layer.
        interp: Interp,
    },
    /// Replaces a whole layered curve, or removes it with `None`. Undo uses
    /// this to restore curves exactly.
    RestoreCurve {
        /// The track.
        track: TrackId,
        /// The parameter.
        param: ParamId,
        /// The curve; `None` removes it.
        curve: Option<LayeredCurve>,
    },
}

/// Changes to a track's settings. `None` fields are unchanged.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
#[allow(missing_docs)]
pub struct TrackPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub singer: Option<SingerRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<BackendRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mix: Option<Mix>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effects: Option<Vec<EffectSlot>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doubling: Option<Doubling>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice_mix: Option<Vec<VoiceMixEntry>>,
}

/// Changes to a note's expression. A missing field is unchanged; for
/// optional settings, `null` removes the setting.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
#[allow(missing_docs)]
pub struct NoteExprPatch {
    #[serde(
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub vibrato: Option<Option<Vibrato>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub portamento: Option<Portamento>,
    #[serde(
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub attack: Option<Option<Envelope>>,
    #[serde(
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub release: Option<Option<Envelope>>,
}

/// Distinguishes a missing field (`None`) from an explicit `null`
/// (`Some(None)`).
fn double_option<'de, T, D>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

impl Op {
    /// A short human-readable name, used as the history label.
    pub fn label(&self) -> String {
        let s = match self {
            Op::Batch { label, .. } => return label.clone(),
            Op::SetMeta { .. } => "Edit project info",
            Op::SetTempo { .. } => "Set tempo",
            Op::RemoveTempo { .. } => "Remove tempo",
            Op::SetTimeSig { .. } => "Set time signature",
            Op::RemoveTimeSig { .. } => "Remove time signature",
            Op::SetKey { .. } => "Set key",
            Op::RemoveKey { .. } => "Remove key",
            Op::SetSeed { .. } => "Set seed",
            Op::SelectTake { .. } => "Select take",
            Op::AddTrack { .. } => "Add track",
            Op::RemoveTrack { .. } => "Remove track",
            Op::MoveTrack { .. } => "Move track",
            Op::SetTrack { .. } => "Edit track",
            Op::AddPart { .. } => "Add part",
            Op::RemovePart { .. } => "Remove part",
            Op::MovePart { .. } => "Move part",
            Op::SetPart { .. } => "Edit part",
            Op::AddNote { .. } => "Add note",
            Op::RemoveNotes { .. } => "Remove notes",
            Op::MoveNotes { .. } => "Move notes",
            Op::ResizeNote { .. } => "Resize note",
            Op::SetLyric { .. } => "Set lyric",
            Op::SetPhonemes { .. } => "Set phonemes",
            Op::SetNoteExpr { .. } => "Edit expression",
            Op::SetVocalModes { .. } => "Set vocal modes",
            Op::ReplaceNotes { .. } => "Replace notes",
            Op::SetCurve { .. } => "Edit curve",
            Op::RestoreCurve { .. } => "Restore curve",
        };
        s.to_owned()
    }
}
