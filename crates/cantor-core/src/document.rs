//! [`Document`]: a project plus its undo tree. All edits go through here.

use crate::apply::apply;
use crate::diag::{Diagnostic, DocDiff};
use crate::history::{History, HistoryNodeId};
use crate::ids::Revision;
use crate::model::Project;
use crate::op::Op;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The outcome of an edit, undo, redo or checkout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ApplyResult {
    /// The document revision afterwards.
    pub revision: Revision,
    /// Whether the document changed. False when an error rejected the edit.
    pub applied: bool,
    /// What changed.
    pub diff: DocDiff,
    /// Errors (edit rejected) and warnings (edit applied). Never silently
    /// dropped.
    pub diagnostics: Vec<Diagnostic>,
}

/// A project being edited.
///
/// The project is held in an [`Arc`] so that [`Document::snapshot`] is
/// cheap; an edit copies the project only while a snapshot is outstanding.
#[derive(Clone, Debug)]
pub struct Document {
    project: Arc<Project>,
    history: History,
    revision: Revision,
}

impl Document {
    /// Opens a project. Its current state becomes the root of the history.
    pub fn new(project: Project) -> Self {
        Self {
            project: Arc::new(project),
            history: History::new(),
            revision: Revision(0),
        }
    }

    /// The current project.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// An immutable snapshot of the current project, for other threads.
    pub fn snapshot(&self) -> Arc<Project> {
        Arc::clone(&self.project)
    }

    /// The current revision.
    pub fn revision(&self) -> Revision {
        self.revision
    }

    /// The undo tree.
    pub fn history(&self) -> &History {
        &self.history
    }

    /// Applies an op atomically and records it in the history.
    pub fn apply(&mut self, op: Op) -> ApplyResult {
        let mut diff = DocDiff::default();
        let mut diagnostics = Vec::new();
        match apply(
            Arc::make_mut(&mut self.project),
            &op,
            &mut diff,
            &mut diagnostics,
        ) {
            Ok(inverse) => {
                self.revision.0 += 1;
                self.history.push(op, inverse, self.revision);
                self.result(true, diff, diagnostics)
            }
            Err(e) => {
                diagnostics.push(e);
                self.result(false, DocDiff::default(), diagnostics)
            }
        }
    }

    /// Reports what `op` would do without changing the document.
    pub fn dry_run(&self, op: &Op) -> ApplyResult {
        let mut scratch = (*self.project).clone();
        let mut diff = DocDiff::default();
        let mut diagnostics = Vec::new();
        let applied = match apply(&mut scratch, op, &mut diff, &mut diagnostics) {
            Ok(_) => true,
            Err(e) => {
                diagnostics.push(e);
                diff = DocDiff::default();
                false
            }
        };
        ApplyResult {
            revision: self.revision,
            applied,
            diff,
            diagnostics,
        }
    }

    /// Undoes the current edit. Returns `None` at the root.
    pub fn undo(&mut self) -> Option<ApplyResult> {
        if !self.history.can_undo() {
            return None;
        }
        let diff = self.step_up();
        self.revision.0 += 1;
        Some(self.result(true, diff, Vec::new()))
    }

    /// Redoes along the most recently visited branch. Returns `None` when
    /// there is nothing to redo.
    pub fn redo(&mut self) -> Option<ApplyResult> {
        let child = self.history.redo_target()?;
        let diff = self.step_down(child);
        self.revision.0 += 1;
        Some(self.result(true, diff, Vec::new()))
    }

    /// Moves to any node of the undo tree, undoing to the common ancestor and
    /// redoing down the target's branch. Returns `None` for an unknown node.
    pub fn checkout(&mut self, target: HistoryNodeId) -> Option<ApplyResult> {
        self.history.node(target)?;
        let (ups, downs) = self.history.path_to(target);
        let mut diff = DocDiff::default();
        for _ in 0..ups {
            diff.merge(self.step_up());
        }
        for child in downs {
            diff.merge(self.step_down(child));
        }
        let changed = !diff.is_empty() || ups > 0;
        if changed {
            self.revision.0 += 1;
        }
        Some(self.result(changed, diff, Vec::new()))
    }

    fn step_up(&mut self) -> DocDiff {
        let node = self
            .history
            .node(self.history.current())
            .expect("current node");
        let inverse = node.inverse.clone().expect("not at root");
        let diff = self.replay(&inverse);
        self.history.step_up();
        diff
    }

    fn step_down(&mut self, child: HistoryNodeId) -> DocDiff {
        let op = self
            .history
            .node(child)
            .and_then(|n| n.op.clone())
            .expect("child op");
        let diff = self.replay(&op);
        self.history.step_down(child);
        diff
    }

    /// Re-applies a recorded op. Recorded ops were valid in exactly this
    /// state, so failure is a bug in an op's inverse.
    fn replay(&mut self, op: &Op) -> DocDiff {
        let mut diff = DocDiff::default();
        apply(
            Arc::make_mut(&mut self.project),
            op,
            &mut diff,
            &mut Vec::new(),
        )
        .unwrap_or_else(|e| panic!("history replay failed: {e:?}"));
        diff
    }

    fn result(&self, applied: bool, diff: DocDiff, diagnostics: Vec<Diagnostic>) -> ApplyResult {
        ApplyResult {
            revision: self.revision,
            applied,
            diff,
            diagnostics,
        }
    }
}
