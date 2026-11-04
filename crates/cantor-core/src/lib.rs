//! Cantor's core: the project model (format v3), [`Op`]s with inverses,
//! undo/redo, tempo math, the parameter and effect registries, phrases
//! and JSON schemas. See docs/design.md.
//!
//! Every edit to a project is an [`Op`], applied with [`apply`] or through
//! a [`Document`] (which keeps the undo history). Clients never mutate a
//! [`Project`] directly.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod apply;
mod curve;
pub mod diag;
pub mod effects;
pub mod history;
pub mod ids;
pub mod model;
pub mod op;
pub mod params;
pub mod phrase;
pub mod pitch;
pub mod schema;
pub mod time;
pub mod tune;
mod validate;

pub use apply::{ApplyResult, apply};
pub use diag::{Diagnostic, Entity, Level, Rejected};
pub use history::{Applied, Document, Step};
pub use ids::{NoteId, PPQ, Revision, Tick, TrackId};
pub use model::*;
pub use op::*;
pub use params::{ParamDescriptor, ParamId};
pub use phrase::{Phrase, phrase_of, phrases};
pub use pitch::{PitchCurve, base_pitch, render_pitch};
pub use time::{TempoMap, TimeSigMap};
pub use tune::{Pass, TuneContext, TuneRequest, TuneResult};

/// Mixes a 64-bit value (SplitMix64 finalizer), for deriving deterministic
/// seeds such as vibrato drift from note ids.
pub fn mix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
