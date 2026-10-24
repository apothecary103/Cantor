//! Cantor's headless core: the document model, the [`Op`] system, the
//! branching undo tree, tempo math and the schema registry.
//!
//! Every edit to a project is an [`Op`] applied through a [`Document`].
//! Clients (GUI, Python, CLI, MCP) never mutate a [`Project`] directly.

#![forbid(unsafe_code)]

mod apply;
mod curve;
pub mod diag;
mod document;
pub mod history;
pub mod ids;
pub mod model;
pub mod op;
pub mod params;
pub mod schema;
pub mod time;

pub use diag::{Diagnostic, DocDiff, EntityRef, Severity};
pub use document::{ApplyResult, Document};
pub use history::{History, HistoryNode, HistoryNodeId};
pub use ids::{NoteId, PPQ, PartId, PhraseId, Revision, TakeId, Tick, TrackId};
pub use model::*;
pub use op::*;
pub use params::{ParamDescriptor, ParamId};
pub use time::{TempoMap, TimeSigMap};
