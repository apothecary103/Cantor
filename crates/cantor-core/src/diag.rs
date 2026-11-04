//! Diagnostics: Cantor never drops an edit silently; anything it cannot
//! honor, or anything wrong with a project, is reported here.
//! JSON: `{ level, code, message, entity? }`.

use crate::ids::{NoteId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

/// How serious a diagnostic is.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Background information.
    Info,
    /// Applied, but something deserves attention.
    Warning,
    /// Rejected; nothing changed.
    Error,
}

/// What a diagnostic is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Entity {
    /// A track.
    Track {
        /// The track.
        id: TrackId,
    },
    /// A note.
    Note {
        /// Its track, when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        track: Option<TrackId>,
        /// The note.
        id: NoteId,
    },
}

/// A message about an edit or a project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Diagnostic {
    /// Severity.
    pub level: Level,
    /// Stable machine-readable code, such as `"note.not_found"`.
    pub code: String,
    /// Human-readable explanation.
    pub message: String,
    /// What it is about, if anything specific.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<Entity>,
}

impl Diagnostic {
    /// Creates a diagnostic about nothing specific.
    pub fn new(level: Level, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            level,
            code: code.into(),
            message: message.into(),
            entity: None,
        }
    }

    /// An error.
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(Level::Error, code, message)
    }

    /// A warning.
    pub fn warning(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(Level::Warning, code, message)
    }

    /// Targets a track (or sets the track of a targeted note).
    pub fn on_track(mut self, track: TrackId) -> Self {
        self.entity = Some(match self.entity {
            Some(Entity::Note { id, .. }) => Entity::Note {
                track: Some(track),
                id,
            },
            _ => Entity::Track { id: track },
        });
        self
    }

    /// Targets a note (keeping a previously targeted track).
    pub fn on_note(mut self, note: NoteId) -> Self {
        let track = match self.entity {
            Some(
                Entity::Track { id }
                | Entity::Note {
                    track: Some(id), ..
                },
            ) => Some(id),
            _ => None,
        };
        self.entity = Some(Entity::Note { track, id: note });
        self
    }

    /// Whether this is an error.
    pub fn is_error(&self) -> bool {
        self.level == Level::Error
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

/// An op that was rejected; the project is unchanged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Rejected {
    /// Why; at least one error.
    pub diagnostics: Vec<Diagnostic>,
}

impl From<Diagnostic> for Rejected {
    fn from(d: Diagnostic) -> Self {
        Self {
            diagnostics: vec![d],
        }
    }
}

impl From<Vec<Diagnostic>> for Rejected {
    fn from(diagnostics: Vec<Diagnostic>) -> Self {
        Self { diagnostics }
    }
}

impl fmt::Display for Rejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msgs: Vec<String> = self.diagnostics.iter().map(ToString::to_string).collect();
        f.write_str(&msgs.join("; "))
    }
}

impl std::error::Error for Rejected {}
