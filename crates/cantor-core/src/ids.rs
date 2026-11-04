//! Identifiers, ticks and revisions.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Pulses per quarter note, the same as UST and USTX.
pub const PPQ: u32 = 480;

/// A position or length in ticks (see [`PPQ`]).
pub type Tick = i64;

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident, $prefix:literal) => {
        $(#[$doc])*
        #[derive(
            Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash,
            Serialize, Deserialize, JsonSchema,
        )]
        #[serde(transparent)]
        pub struct $name(pub u64);

        impl $name {
            /// The placeholder id `0`: "assign one for me" in `add_*` ops.
            pub const UNASSIGNED: Self = Self(0);

            /// Whether this is the placeholder id `0`.
            pub fn is_unassigned(self) -> bool {
                self.0 == 0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!($prefix, "{}"), self.0)
            }
        }
    };
}

id_type!(
    /// Identifies a track. `0` (or omitted) in `add_track` means "assign one".
    TrackId,
    "track#"
);
id_type!(
    /// Identifies a note, unique within the project. `0` (or omitted) in
    /// `add_notes` means "assign one".
    NoteId,
    "note#"
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
