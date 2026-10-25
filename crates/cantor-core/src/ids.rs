//! Identifiers, ticks and revisions.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Pulses per quarter note. 1920 = 2^7 · 3 · 5, so triplets and
/// quintuplets of every common note value land on whole ticks.
pub const PPQ: u32 = 1920;

/// A position or length in ticks (see [`PPQ`]).
pub type Tick = i64;

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident, $prefix:literal) => {
        $(#[$doc])*
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
            Serialize, Deserialize, JsonSchema,
        )]
        #[serde(transparent)]
        pub struct $name(pub u64);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!($prefix, "{}"), self.0)
            }
        }
    };
}

id_type!(
    /// Identifies a track.
    TrackId,
    "track#"
);
id_type!(
    /// Identifies a part (a region of notes on a track).
    PartId,
    "part#"
);
id_type!(
    /// Identifies a note.
    NoteId,
    "note#"
);
id_type!(
    /// Identifies a phrase, the unit of rendering, seeding and retakes.
    PhraseId,
    "phrase#"
);
id_type!(
    /// Identifies a take within a phrase's take lane.
    TakeId,
    "take#"
);

/// A document revision. It increases on every change, including undo and
/// redo, so it identifies a state for optimistic concurrency.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(transparent)]
pub struct Revision(pub u64);

impl fmt::Display for Revision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
