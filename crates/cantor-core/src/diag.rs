//! Diagnostics and document diffs.

use crate::ids::{NoteId, PartId, PhraseId, TrackId};
use crate::params::ParamId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// How serious a diagnostic is.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Background information.
    Info,
    /// The edit was applied, but something deserves attention.
    Warning,
    /// The edit was rejected; nothing changed.
    Error,
}

/// A message about an edit, import or file. Cantor never drops an edit
/// silently: anything it cannot honor is reported here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Diagnostic {
    /// Severity.
    pub severity: Severity,
    /// Stable machine-readable code, such as `"note.not_found"`.
    pub code: String,
    /// Human-readable explanation.
    pub message: String,
    /// The entity concerned, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<EntityRef>,
}

impl Diagnostic {
    /// Creates a diagnostic.
    pub fn new(
        severity: Severity,
        code: impl Into<String>,
        message: impl Into<String>,
        target: Option<EntityRef>,
    ) -> Self {
        Self {
            severity,
            code: code.into(),
            message: message.into(),
            target,
        }
    }

    /// An error diagnostic.
    pub fn error(
        code: impl Into<String>,
        message: impl Into<String>,
        target: Option<EntityRef>,
    ) -> Self {
        Self::new(Severity::Error, code, message, target)
    }

    /// A warning diagnostic.
    pub fn warning(
        code: impl Into<String>,
        message: impl Into<String>,
        target: Option<EntityRef>,
    ) -> Self {
        Self::new(Severity::Warning, code, message, target)
    }

    /// An info diagnostic.
    pub fn info(
        code: impl Into<String>,
        message: impl Into<String>,
        target: Option<EntityRef>,
    ) -> Self {
        Self::new(Severity::Info, code, message, target)
    }

    /// Whether this is an error.
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

/// Points at a part of the document.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum EntityRef {
    Meta,
    TempoMap,
    TimeSigMap,
    KeyMap,
    Track { id: TrackId },
    Part { id: PartId },
    Note { id: NoteId },
    Curve { track: TrackId, param: ParamId },
    Phrase { id: PhraseId },
    Section { name: String },
}

/// What an applied edit changed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocDiff {
    /// Entities that now exist and did not before.
    pub added: BTreeSet<EntityRef>,
    /// Entities that existed and no longer do.
    pub removed: BTreeSet<EntityRef>,
    /// Entities that exist before and after, with changed content.
    pub modified: BTreeSet<EntityRef>,
}

impl DocDiff {
    /// Whether nothing changed.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.modified.is_empty()
    }

    pub(crate) fn add(&mut self, e: EntityRef) {
        if !self.removed.remove(&e) {
            self.added.insert(e);
        } else {
            self.modified.insert(e);
        }
    }

    pub(crate) fn remove(&mut self, e: EntityRef) {
        self.modified.remove(&e);
        if !self.added.remove(&e) {
            self.removed.insert(e);
        }
    }

    pub(crate) fn modify(&mut self, e: EntityRef) {
        if !self.added.contains(&e) {
            self.modified.insert(e);
        }
    }

    /// Folds a later diff into this one.
    pub fn merge(&mut self, later: DocDiff) {
        for e in later.removed {
            self.remove(e);
        }
        for e in later.added {
            self.add(e);
        }
        for e in later.modified {
            self.modify(e);
        }
    }
}
