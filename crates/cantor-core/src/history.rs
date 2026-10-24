//! The undo tree. Undoing and then making a new edit starts a branch; the
//! old branch is kept and can be checked out again.

use crate::ids::Revision;
use crate::op::Op;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Identifies a node in the undo tree. The root is node 0.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct HistoryNodeId(pub u32);

impl HistoryNodeId {
    /// The root: the document as it was opened.
    pub const ROOT: Self = Self(0);
}

/// One edit in the undo tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HistoryNode {
    /// This node.
    pub id: HistoryNodeId,
    /// The state this edit was applied to; `None` for the root.
    pub parent: Option<HistoryNodeId>,
    /// Edits made from this state, oldest first. More than one is a branch.
    pub children: Vec<HistoryNodeId>,
    /// The child redo follows: the most recently visited branch.
    pub redo_child: Option<HistoryNodeId>,
    /// Name shown in the history panel.
    pub label: String,
    /// The edit; `None` for the root.
    pub op: Option<Op>,
    /// Restores the parent state; `None` for the root.
    pub inverse: Option<Op>,
    /// The revision at which this edit was first applied.
    pub revision: Revision,
}

/// The undo tree and the current position in it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct History {
    nodes: Vec<HistoryNode>,
    current: HistoryNodeId,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    /// A tree holding only the root.
    pub fn new() -> Self {
        Self {
            nodes: vec![HistoryNode {
                id: HistoryNodeId::ROOT,
                parent: None,
                children: Vec::new(),
                redo_child: None,
                label: "Open".into(),
                op: None,
                inverse: None,
                revision: Revision(0),
            }],
            current: HistoryNodeId::ROOT,
        }
    }

    /// The node for the current state.
    pub fn current(&self) -> HistoryNodeId {
        self.current
    }

    /// Looks up a node.
    pub fn node(&self, id: HistoryNodeId) -> Option<&HistoryNode> {
        self.nodes.get(id.0 as usize)
    }

    /// All nodes, in creation order.
    pub fn nodes(&self) -> &[HistoryNode] {
        &self.nodes
    }

    /// Whether there is an edit to undo.
    pub fn can_undo(&self) -> bool {
        self.current != HistoryNodeId::ROOT
    }

    /// The child redo would follow, if any.
    pub fn redo_target(&self) -> Option<HistoryNodeId> {
        let n = &self.nodes[self.current.0 as usize];
        n.redo_child.or_else(|| n.children.last().copied())
    }

    /// Records an applied edit as a new child of the current node and moves
    /// to it.
    pub(crate) fn push(&mut self, op: Op, inverse: Op, revision: Revision) -> HistoryNodeId {
        let id = HistoryNodeId(u32::try_from(self.nodes.len()).expect("fewer than 2^32 edits"));
        let parent = self.current;
        self.nodes.push(HistoryNode {
            id,
            parent: Some(parent),
            children: Vec::new(),
            redo_child: None,
            label: op.label(),
            op: Some(op),
            inverse: Some(inverse),
            revision,
        });
        let p = &mut self.nodes[parent.0 as usize];
        p.children.push(id);
        p.redo_child = Some(id);
        self.current = id;
        id
    }

    /// Moves to the parent after its inverse was applied.
    pub(crate) fn step_up(&mut self) {
        let cur = self.current;
        let parent = self.nodes[cur.0 as usize].parent.expect("not at root");
        self.nodes[parent.0 as usize].redo_child = Some(cur);
        self.current = parent;
    }

    /// Moves to a child after its op was re-applied.
    pub(crate) fn step_down(&mut self, child: HistoryNodeId) {
        debug_assert_eq!(self.nodes[child.0 as usize].parent, Some(self.current));
        self.nodes[self.current.0 as usize].redo_child = Some(child);
        self.current = child;
    }

    /// The path from the current node to `target`: how many steps up to
    /// the common ancestor, then the nodes to step down through.
    pub(crate) fn path_to(&self, target: HistoryNodeId) -> (usize, Vec<HistoryNodeId>) {
        let ancestors = |mut n: HistoryNodeId| {
            let mut v = vec![n];
            while let Some(p) = self.nodes[n.0 as usize].parent {
                v.push(p);
                n = p;
            }
            v
        };
        let from = ancestors(self.current);
        let to = ancestors(target);
        let lca = *from
            .iter()
            .find(|n| to.contains(n))
            .expect("the root is a common ancestor");
        let ups = from.iter().position(|n| *n == lca).expect("lca on path");
        let mut downs: Vec<_> = to.into_iter().take_while(|n| *n != lca).collect();
        downs.reverse();
        (ups, downs)
    }
}
