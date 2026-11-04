//! [`Document`]: a project with linear undo/redo and named checkpoints.
//! All edits go through [`Document::apply`].

use crate::apply::{apply, apply_checked};
use crate::diag::{Diagnostic, Rejected};
use crate::ids::Revision;
use crate::model::Project;
use crate::op::Op;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One undo or redo step: the op that performs it and its label.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    /// What the user did, e.g. "Move notes".
    pub label: String,
    /// The op that undoes (on the undo stack) or redoes (on the redo
    /// stack) it.
    pub op: Op,
}

/// The result of an edit, undo, redo or restore.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Applied {
    /// The document revision afterwards.
    pub revision: Revision,
    /// The label of the step.
    pub label: String,
    /// Warnings about the new state.
    pub diagnostics: Vec<Diagnostic>,
}

/// A project being edited.
#[derive(Clone, Debug)]
pub struct Document {
    project: Project,
    undo: Vec<Step>,
    redo: Vec<Step>,
    checkpoints: Vec<(String, Project)>,
    revision: Revision,
    saved: Revision,
}

impl Document {
    /// Opens a project (canonicalizing it) with empty history. Check
    /// [`Project::validate`] first: ops refuse to recreate invalid notes.
    pub fn new(mut project: Project) -> Self {
        project.canonicalize();
        Self {
            project,
            undo: Vec::new(),
            redo: Vec::new(),
            checkpoints: Vec::new(),
            revision: Revision(0),
            saved: Revision(0),
        }
    }

    /// The current project.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// The current revision; it increases on every change.
    pub fn revision(&self) -> Revision {
        self.revision
    }

    /// Applies an op as one undo step and clears the redo stack.
    pub fn apply(&mut self, op: Op) -> Result<Applied, Rejected> {
        let label = op.label();
        let r = apply(&mut self.project, &op)?;
        self.undo.push(Step {
            label: label.clone(),
            op: r.inverse,
        });
        self.redo.clear();
        Ok(self.bump(label, r.diagnostics))
    }

    /// Undoes the last step. `Ok(None)` when there is nothing to undo.
    pub fn undo(&mut self) -> Result<Option<Applied>, Rejected> {
        Self::step(&mut self.project, &mut self.undo, &mut self.redo)
            .map(|s| s.map(|(label, d)| self.bump(label, d)))
    }

    /// Redoes the last undone step. `Ok(None)` when there is nothing to redo.
    pub fn redo(&mut self) -> Result<Option<Applied>, Rejected> {
        Self::step(&mut self.project, &mut self.redo, &mut self.undo)
            .map(|s| s.map(|(label, d)| self.bump(label, d)))
    }

    /// Pops a step from `from`, applies it and pushes its inverse to `to`.
    fn step(
        project: &mut Project,
        from: &mut Vec<Step>,
        to: &mut Vec<Step>,
    ) -> Result<Option<(String, Vec<Diagnostic>)>, Rejected> {
        let Some(step) = from.last() else {
            return Ok(None);
        };
        let r = apply_checked(project, &step.op, false)?;
        let label = from.pop().expect("non-empty").label;
        to.push(Step {
            label: label.clone(),
            op: r.inverse,
        });
        Ok(Some((label, r.diagnostics)))
    }

    fn bump(&mut self, label: String, diagnostics: Vec<Diagnostic>) -> Applied {
        self.revision.0 += 1;
        Applied {
            revision: self.revision,
            label,
            diagnostics,
        }
    }

    /// Whether there is a step to undo.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether there is a step to redo.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Labels of the undo stack, oldest first.
    pub fn undo_labels(&self) -> impl Iterator<Item = &str> {
        self.undo.iter().map(|s| s.label.as_str())
    }

    /// Labels of the redo stack, next redo last.
    pub fn redo_labels(&self) -> impl Iterator<Item = &str> {
        self.redo.iter().map(|s| s.label.as_str())
    }

    /// Saves a snapshot of the current project under `name`, replacing any
    /// checkpoint of that name.
    pub fn checkpoint(&mut self, name: impl Into<String>) {
        let name = name.into();
        self.checkpoints.retain(|(n, _)| *n != name);
        self.checkpoints.push((name, self.project.clone()));
    }

    /// Checkpoint names, oldest first.
    pub fn checkpoints(&self) -> impl Iterator<Item = &str> {
        self.checkpoints.iter().map(|(n, _)| n.as_str())
    }

    /// Returns to a checkpoint as one undoable step.
    pub fn restore(&mut self, name: &str) -> Result<Applied, Rejected> {
        let snapshot = self
            .checkpoints
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, p)| p)
            .ok_or_else(|| {
                Rejected::from(Diagnostic::error(
                    "checkpoint.not_found",
                    format!("no checkpoint `{name}`"),
                ))
            })?;
        let ops = Op::transition(&self.project, snapshot);
        self.apply(Op::batch(format!("Restore `{name}`"), ops))
    }

    /// Replaces the whole project as one undoable step (e.g. an import).
    pub fn replace(&mut self, mut project: Project, label: &str) -> Result<Applied, Rejected> {
        project.canonicalize();
        let ops = Op::transition(&self.project, &project);
        self.apply(Op::batch(label, ops))
    }

    /// Records that the current state was saved.
    pub fn mark_saved(&mut self) {
        self.saved = self.revision;
    }

    /// Whether anything changed since the last save (or since opening).
    pub fn is_dirty(&self) -> bool {
        self.saved != self.revision
    }
}
