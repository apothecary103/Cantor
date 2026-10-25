//! The action registry: every GUI action, with the Op it applies and its
//! Python equivalent. The command palette lists this registry; the Script
//! Log shows each applied Op as a Python line; a test checks that every
//! edit action builds a valid Op and names an existing Python method.

use cantor_core::{
    Interp, Note, NoteExprPatch, NoteId, Op, ParamId, PhraseId, Portamento, Project, RetakeDim,
    TakeId, TickRange, TrackId, TrackPatch, Vibrato,
};

/// What an action does to the project.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Applies an Op (the `op` tag of [`ActionDef::op`]).
    Edit,
    /// Moves through the undo tree (undo, redo, checkpoints).
    History,
    /// Changes only the view (zoom, panels, palette) or renders.
    View,
}

/// Where an action applies: the editor's current selection and cursor.
#[derive(Clone, Debug, Default)]
pub struct ActionContext {
    /// Selected notes, in time order.
    pub selection: Vec<NoteId>,
    /// The focused track.
    pub track: Option<TrackId>,
    /// Cursor position (absolute tick) and pitch (cents).
    pub cursor_tick: i64,
    /// Cursor pitch in cents.
    pub cursor_cents: f64,
    /// Grid step in ticks.
    pub grid: i64,
    /// Text typed for actions that take it (lyrics, names).
    pub text: String,
    /// Parameter lane in focus.
    pub param: Option<String>,
    /// Phrase under the cursor.
    pub phrase: Option<PhraseId>,
}

/// A registered action.
#[derive(Clone, Copy)]
pub struct ActionDef {
    /// Stable id, used for key bindings and macros.
    pub id: &'static str,
    /// Name shown in the command palette.
    pub name: &'static str,
    /// Default key bindings (GPUI keystroke syntax).
    pub keys: &'static [&'static str],
    /// Edit, history or view.
    pub kind: Kind,
    /// For edits: the Op tag it applies (see the op JSON Schema).
    pub op: Option<&'static str>,
    /// The Python method that does the same (see cantor/__init__.pyi).
    pub python: &'static str,
    /// Builds the Op for an edit in a context; `None` if it does not apply.
    pub build: Option<fn(&Project, &ActionContext) -> Option<Op>>,
}

impl std::fmt::Debug for ActionDef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActionDef")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

fn selection_bounds(p: &Project, ctx: &ActionContext) -> Option<TickRange> {
    let mut lo = i64::MAX;
    let mut hi = i64::MIN;
    for id in &ctx.selection {
        let (_, part, n) = p.note(*id)?;
        lo = lo.min(part.start + n.start);
        hi = hi.max(part.start + n.start + n.length);
    }
    (lo < hi).then_some(TickRange { start: lo, end: hi })
}

fn first_selected<'a>(p: &'a Project, ctx: &ActionContext) -> Option<&'a Note> {
    ctx.selection
        .first()
        .and_then(|id| p.note(*id))
        .map(|(_, _, n)| n)
}

fn move_by(dt: i64, dcents: f64) -> impl Fn(&Project, &ActionContext) -> Option<Op> {
    move |_, ctx| {
        (!ctx.selection.is_empty()).then(|| Op::MoveNotes {
            ids: ctx.selection.clone(),
            dt: dt * ctx.grid.max(1),
            dcents,
            carry_curves: true,
        })
    }
}

macro_rules! mover {
    ($name:ident, $dt:expr, $dc:expr) => {
        fn $name(p: &Project, ctx: &ActionContext) -> Option<Op> {
            move_by($dt, $dc)(p, ctx)
        }
    };
}
mover!(move_left, -1, 0.0);
mover!(move_right, 1, 0.0);
mover!(move_up, 0, 100.0);
mover!(move_down, 0, -100.0);
mover!(octave_up, 0, 1200.0);
mover!(octave_down, 0, -1200.0);

fn add_note(p: &Project, ctx: &ActionContext) -> Option<Op> {
    let track = p.track(ctx.track?)?;
    let part = track
        .parts
        .iter()
        .find(|x| ctx.cursor_tick >= x.start && ctx.cursor_tick < x.start + x.length)
        .or(track.parts.first())?;
    let grid = ctx.grid.max(1);
    let start = (ctx.cursor_tick - part.start).div_euclid(grid) * grid;
    Some(Op::AddNote {
        part: part.id,
        note: Note {
            id: p.next_note_id(),
            start: start.max(0),
            length: grid,
            pitch_cents: (ctx.cursor_cents / 100.0).round() * 100.0,
            lyric: if ctx.text.is_empty() {
                "la".into()
            } else {
                ctx.text.clone()
            },
            lang: None,
            phonemes: None,
            expr: Default::default(),
            modes: Default::default(),
        },
    })
}

fn delete_notes(_: &Project, ctx: &ActionContext) -> Option<Op> {
    (!ctx.selection.is_empty()).then(|| Op::RemoveNotes {
        ids: ctx.selection.clone(),
    })
}

fn lengthen(p: &Project, ctx: &ActionContext) -> Option<Op> {
    let n = first_selected(p, ctx)?;
    Some(Op::ResizeNote {
        id: n.id,
        length: n.length + ctx.grid.max(1),
    })
}

fn shorten(p: &Project, ctx: &ActionContext) -> Option<Op> {
    let n = first_selected(p, ctx)?;
    Some(Op::ResizeNote {
        id: n.id,
        length: (n.length - ctx.grid.max(1)).max(1),
    })
}

/// Typed lyrics flow across the selection, one syllable per note.
fn set_lyric(p: &Project, ctx: &ActionContext) -> Option<Op> {
    let words: Vec<&str> = ctx.text.split_whitespace().collect();
    if ctx.selection.is_empty() || words.is_empty() {
        return None;
    }
    let ops: Vec<Op> = ctx
        .selection
        .iter()
        .zip(words.iter().chain(std::iter::repeat(&"+")))
        .filter(|(id, _)| p.note(**id).is_some())
        .map(|(id, w)| Op::SetLyric {
            id: *id,
            lyric: (*w).to_owned(),
        })
        .collect();
    match ops.len() {
        0 => None,
        1 => ops.into_iter().next(),
        _ => Some(Op::Batch {
            label: "Enter lyrics".into(),
            ops,
        }),
    }
}

fn clear_phonemes(p: &Project, ctx: &ActionContext) -> Option<Op> {
    let n = first_selected(p, ctx)?;
    Some(Op::SetPhonemes {
        id: n.id,
        phonemes: None,
    })
}

fn add_vibrato(p: &Project, ctx: &ActionContext) -> Option<Op> {
    let n = first_selected(p, ctx)?;
    Some(Op::SetNoteExpr {
        id: n.id,
        expr: NoteExprPatch {
            vibrato: Some(Some(Vibrato {
                depth_cents: 40.0,
                rate_hz: 5.5,
                start: 0.3,
                fade_in: 0.25,
                fade_out: 0.15,
                phase: 0.0,
                drift: 0.5,
            })),
            ..Default::default()
        },
    })
}

fn remove_vibrato(p: &Project, ctx: &ActionContext) -> Option<Op> {
    let n = first_selected(p, ctx)?;
    Some(Op::SetNoteExpr {
        id: n.id,
        expr: NoteExprPatch {
            vibrato: Some(None),
            ..Default::default()
        },
    })
}

fn slower_portamento(p: &Project, ctx: &ActionContext) -> Option<Op> {
    let n = first_selected(p, ctx)?;
    let len = n.expr.portamento.length_ms.unwrap_or(120.0);
    Some(Op::SetNoteExpr {
        id: n.id,
        expr: NoteExprPatch {
            portamento: Some(Portamento {
                length_ms: Some((len * 1.25).min(1000.0)),
                ..n.expr.portamento.clone()
            }),
            ..Default::default()
        },
    })
}

fn clear_curve(p: &Project, ctx: &ActionContext) -> Option<Op> {
    let range = selection_bounds(p, ctx)?;
    Some(Op::SetCurve {
        track: ctx.track?,
        param: ParamId::new(ctx.param.clone().unwrap_or_else(|| "pitch".into())),
        range,
        points: Vec::new(),
        interp: Interp::Linear,
    })
}

fn retake(p: &Project, ctx: &ActionContext, dims: Vec<RetakeDim>) -> Option<Op> {
    let range = selection_bounds(p, ctx).or_else(|| {
        let t = p.track(ctx.track?)?;
        cantor_core::phrase_spans(p, t)
            .into_iter()
            .find(|s| ctx.cursor_tick >= s.start && ctx.cursor_tick < s.end)
            .map(|s| TickRange {
                start: s.start,
                end: s.end,
            })
    })?;
    Some(Op::Retake {
        track: ctx.track?,
        range,
        dims,
        n_variants: 1,
        seed: None,
    })
}

fn retake_pitch(p: &Project, ctx: &ActionContext) -> Option<Op> {
    retake(p, ctx, vec![RetakeDim::Pitch])
}

fn retake_all(p: &Project, ctx: &ActionContext) -> Option<Op> {
    retake(
        p,
        ctx,
        vec![RetakeDim::Pitch, RetakeDim::Timbre, RetakeDim::Expression],
    )
}

/// Cycles the take of the phrase under the cursor (original → 1 → 2 → …).
fn cycle_take(p: &Project, ctx: &ActionContext) -> Option<Op> {
    let phrase = ctx.phrase?;
    let lane = p.takes.get(&phrase)?;
    let next = match lane.selected {
        None => lane.takes.first().map(|t| t.id),
        Some(cur) => {
            let i = lane.takes.iter().position(|t| t.id == cur)?;
            lane.takes.get(i + 1).map(|t| t.id)
        }
    };
    Some(Op::SelectTake { phrase, take: next })
}

fn toggle_mute(p: &Project, ctx: &ActionContext) -> Option<Op> {
    let t = p.track(ctx.track?)?;
    let mut mix = t.mix.clone();
    mix.mute = !mix.mute;
    Some(Op::SetTrack {
        id: t.id,
        patch: TrackPatch {
            mix: Some(mix),
            ..Default::default()
        },
    })
}

fn set_section(p: &Project, ctx: &ActionContext) -> Option<Op> {
    let range = selection_bounds(p, ctx)?;
    let name = if ctx.text.is_empty() {
        "section".to_owned()
    } else {
        ctx.text.clone()
    };
    Some(Op::SetSection {
        name,
        start: range.start,
        end: range.end,
    })
}

/// A style prompt over the selection (or the whole track).
fn style_apply(p: &Project, ctx: &ActionContext) -> Option<Op> {
    let track = ctx.track?;
    let range = selection_bounds(p, ctx).or_else(|| {
        let t = p.track(track)?;
        Some(TickRange {
            start: t.parts.iter().map(|x| x.start).min()?,
            end: t.parts.iter().map(|x| x.start + x.length).max()?,
        })
    })?;
    let plan = cantor_core::style::interpret(&ctx.text);
    cantor_core::style::ops(p, track, range, &ctx.text, &plan).0
}

fn _unused(_: TakeId) {}

/// The registry, in palette order.
pub const REGISTRY: &[ActionDef] = &[
    ActionDef {
        id: "note.add",
        name: "Add note at cursor",
        keys: &["n"],
        kind: Kind::Edit,
        op: Some("add_note"),
        python: "Track.add_note",
        build: Some(add_note),
    },
    ActionDef {
        id: "note.delete",
        name: "Delete selected notes",
        keys: &["backspace", "delete"],
        kind: Kind::Edit,
        op: Some("remove_notes"),
        python: "Note.remove",
        build: Some(delete_notes),
    },
    ActionDef {
        id: "note.left",
        name: "Move notes left one grid step",
        keys: &["left"],
        kind: Kind::Edit,
        op: Some("move_notes"),
        python: "Note.move_by",
        build: Some(move_left),
    },
    ActionDef {
        id: "note.right",
        name: "Move notes right one grid step",
        keys: &["right"],
        kind: Kind::Edit,
        op: Some("move_notes"),
        python: "Note.move_by",
        build: Some(move_right),
    },
    ActionDef {
        id: "note.up",
        name: "Transpose up a semitone",
        keys: &["up"],
        kind: Kind::Edit,
        op: Some("move_notes"),
        python: "Note.move_by",
        build: Some(move_up),
    },
    ActionDef {
        id: "note.down",
        name: "Transpose down a semitone",
        keys: &["down"],
        kind: Kind::Edit,
        op: Some("move_notes"),
        python: "Note.move_by",
        build: Some(move_down),
    },
    ActionDef {
        id: "note.octave_up",
        name: "Transpose up an octave",
        keys: &["shift-up"],
        kind: Kind::Edit,
        op: Some("move_notes"),
        python: "Note.move_by",
        build: Some(octave_up),
    },
    ActionDef {
        id: "note.octave_down",
        name: "Transpose down an octave",
        keys: &["shift-down"],
        kind: Kind::Edit,
        op: Some("move_notes"),
        python: "Note.move_by",
        build: Some(octave_down),
    },
    ActionDef {
        id: "note.lengthen",
        name: "Lengthen note by one grid step",
        keys: &["shift-right"],
        kind: Kind::Edit,
        op: Some("resize_note"),
        python: "Note.resize",
        build: Some(lengthen),
    },
    ActionDef {
        id: "note.shorten",
        name: "Shorten note by one grid step",
        keys: &["shift-left"],
        kind: Kind::Edit,
        op: Some("resize_note"),
        python: "Note.resize",
        build: Some(shorten),
    },
    ActionDef {
        id: "lyric.enter",
        name: "Enter lyrics across selection",
        keys: &["enter"],
        kind: Kind::Edit,
        op: Some("set_lyric"),
        python: "Note.lyric",
        build: Some(set_lyric),
    },
    ActionDef {
        id: "phonemes.clear",
        name: "Clear phoneme overrides",
        keys: &[],
        kind: Kind::Edit,
        op: Some("set_phonemes"),
        python: "Note.set_phonemes",
        build: Some(clear_phonemes),
    },
    ActionDef {
        id: "expr.vibrato",
        name: "Add vibrato",
        keys: &["v"],
        kind: Kind::Edit,
        op: Some("set_note_expr"),
        python: "Note.vibrato",
        build: Some(add_vibrato),
    },
    ActionDef {
        id: "expr.no_vibrato",
        name: "Remove vibrato",
        keys: &["shift-v"],
        kind: Kind::Edit,
        op: Some("set_note_expr"),
        python: "Note.vibrato",
        build: Some(remove_vibrato),
    },
    ActionDef {
        id: "expr.slower_portamento",
        name: "Lengthen transition into note",
        keys: &["p"],
        kind: Kind::Edit,
        op: Some("set_note_expr"),
        python: "Note.portamento",
        build: Some(slower_portamento),
    },
    ActionDef {
        id: "curve.clear",
        name: "Clear curve edits under selection",
        keys: &["shift-backspace"],
        kind: Kind::Edit,
        op: Some("set_curve"),
        python: "Track.clear_curve",
        build: Some(clear_curve),
    },
    ActionDef {
        id: "take.retake_pitch",
        name: "Retake pitch",
        keys: &["r"],
        kind: Kind::Edit,
        op: Some("retake"),
        python: "Track.retake",
        build: Some(retake_pitch),
    },
    ActionDef {
        id: "take.retake",
        name: "Retake pitch, timbre and expression",
        keys: &["shift-r"],
        kind: Kind::Edit,
        op: Some("retake"),
        python: "Track.retake",
        build: Some(retake_all),
    },
    ActionDef {
        id: "take.cycle",
        name: "Cycle take",
        keys: &["t"],
        kind: Kind::Edit,
        op: Some("select_take"),
        python: "Project.select_take",
        build: Some(cycle_take),
    },
    ActionDef {
        id: "track.mute",
        name: "Toggle track mute",
        keys: &["m"],
        kind: Kind::Edit,
        op: Some("set_track"),
        python: "Track.set_mix",
        build: Some(toggle_mute),
    },
    ActionDef {
        id: "section.set",
        name: "Mark selection as section",
        keys: &[],
        kind: Kind::Edit,
        op: Some("set_section"),
        python: "Project.set_section",
        build: Some(set_section),
    },
    ActionDef {
        id: "style.apply",
        name: "Style prompt…",
        keys: &["cmd-shift-y", "ctrl-shift-y"],
        kind: Kind::Edit,
        op: Some("set_curve"),
        python: "Track.style",
        build: Some(style_apply),
    },
    ActionDef {
        id: "history.undo",
        name: "Undo",
        keys: &["cmd-z", "ctrl-z"],
        kind: Kind::History,
        op: None,
        python: "Project.undo",
        build: None,
    },
    ActionDef {
        id: "history.redo",
        name: "Redo",
        keys: &["cmd-shift-z", "ctrl-shift-z"],
        kind: Kind::History,
        op: None,
        python: "Project.redo",
        build: None,
    },
    ActionDef {
        id: "history.checkpoint",
        name: "Save checkpoint",
        keys: &["cmd-shift-s"],
        kind: Kind::History,
        op: None,
        python: "Project.checkpoint",
        build: None,
    },
    ActionDef {
        id: "render.preview",
        name: "Render and analyze",
        keys: &["cmd-r", "ctrl-r"],
        kind: Kind::View,
        op: None,
        python: "Project.render",
        build: None,
    },
    ActionDef {
        id: "bridge.publish",
        name: "Send to DAW (bridge)",
        keys: &["cmd-shift-b", "ctrl-shift-b"],
        kind: Kind::View,
        op: None,
        python: "Project.publish_to_bridge",
        build: None,
    },
    ActionDef {
        id: "view.palette",
        name: "Command palette",
        keys: &["cmd-k", "ctrl-k"],
        kind: Kind::View,
        op: None,
        python: "",
        build: None,
    },
    ActionDef {
        id: "view.zoom_in",
        name: "Zoom in",
        keys: &["cmd-=", "ctrl-="],
        kind: Kind::View,
        op: None,
        python: "",
        build: None,
    },
    ActionDef {
        id: "view.zoom_out",
        name: "Zoom out",
        keys: &["cmd--", "ctrl--"],
        kind: Kind::View,
        op: None,
        python: "",
        build: None,
    },
    ActionDef {
        id: "view.select_all",
        name: "Select all notes",
        keys: &["cmd-a", "ctrl-a"],
        kind: Kind::View,
        op: None,
        python: "Track.notes",
        build: None,
    },
    ActionDef {
        id: "macro.toggle",
        name: "Start or stop macro recording",
        keys: &["cmd-shift-m"],
        kind: Kind::View,
        op: None,
        python: "",
        build: None,
    },
];

/// Looks up an action.
pub fn find(id: &str) -> Option<&'static ActionDef> {
    REGISTRY.iter().find(|a| a.id == id)
}

/// The Python line equivalent to an applied op (for the Script Log and
/// macros). `p` is the project before the op.
pub fn python_line(op: &Op, p: &Project) -> String {
    let q = |s: &str| format!("{s:?}");
    match op {
        Op::Batch { label, ops } if label.starts_with("Style: ") => {
            // A style prompt is one call: track and range from its edits.
            let prompt = &label["Style: ".len()..];
            let (track, range) = ops
                .iter()
                .find_map(|o| match o {
                    Op::SetCurve { track, range, .. } => Some((track.0, *range)),
                    _ => None,
                })
                .unwrap_or((0, TickRange { start: 0, end: 0 }));
            format!(
                "p.track({track}).style({}, start={}, end={})",
                q(prompt),
                range.start,
                range.end
            )
        }
        Op::Batch { label, ops } => {
            let mut lines = vec![format!("with p.transaction({}):", q(label))];
            lines.extend(ops.iter().map(|o| format!("    {}", python_line(o, p))));
            lines.join("\n")
        }
        Op::AddNote { part, note } => {
            let (track, pt) = p.part(*part).map_or((0, 0), |(t, x)| (t.id.0, x.start));
            format!(
                "p.track({track}).add_note(start={}, length={}, pitch_cents={}, lyric={})",
                pt + note.start,
                note.length,
                note.pitch_cents,
                q(&note.lyric)
            )
        }
        Op::RemoveNotes { ids } => ids
            .iter()
            .map(|i| format!("p.note({}).remove()", i.0))
            .collect::<Vec<_>>()
            .join("\n"),
        Op::MoveNotes {
            ids, dt, dcents, ..
        } => ids
            .iter()
            .map(|i| format!("p.note({}).move_by(dt={dt}, dcents={dcents})", i.0))
            .collect::<Vec<_>>()
            .join("\n"),
        Op::ResizeNote { id, length } => format!("p.note({}).resize({length})", id.0),
        Op::SetLyric { id, lyric } => format!("p.note({}).lyric = {}", id.0, q(lyric)),
        Op::SetPhonemes { id, phonemes: None } => format!("p.note({}).set_phonemes(None)", id.0),
        Op::SetPhonemes {
            id,
            phonemes: Some(ph),
        } => format!(
            "p.note({}).set_phonemes([{}], lengths_ms=[{}])",
            id.0,
            ph.iter()
                .map(|x| q(&x.symbol))
                .collect::<Vec<_>>()
                .join(", "),
            ph.iter()
                .map(|x| x.length_ms.map_or("None".into(), |l| l.to_string()))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Op::SetNoteExpr { id, expr } => {
            let mut parts = Vec::new();
            if let Some(v) = &expr.vibrato {
                parts.push(match v {
                    Some(v) => format!(
                        "p.note({}).vibrato(depth_cents={}, rate_hz={}, start={}, fade_in={}, fade_out={}, drift={})",
                        id.0, v.depth_cents, v.rate_hz, v.start, v.fade_in, v.fade_out, v.drift
                    ),
                    None => format!("p.note({}).vibrato(depth_cents=0, rate_hz=0)", id.0),
                });
            }
            if let Some(pt) = &expr.portamento {
                parts.push(format!(
                    "p.note({}).portamento(length_ms={}, overshoot_cents={}, preparation_cents={}, before={})",
                    id.0,
                    pt.length_ms.map_or("None".into(), |l| l.to_string()),
                    pt.overshoot_cents,
                    pt.preparation_cents,
                    pt.before
                ));
            }
            if parts.is_empty() {
                format!("# expression change on note {}", id.0)
            } else {
                parts.join("\n")
            }
        }
        Op::SetCurve {
            track,
            param,
            range,
            points,
            interp,
        } => {
            let interp = match interp {
                Interp::Step => "step",
                Interp::Linear => "linear",
                Interp::MonotoneHermite => "hermite",
            };
            if points.is_empty() {
                format!(
                    "p.track({}).clear_curve({}, {}, {})",
                    track.0,
                    q(param.as_str()),
                    range.start,
                    range.end
                )
            } else {
                format!(
                    "p.track({}).set_curve({}, {}, {}, {:?}, interp={})",
                    track.0,
                    q(param.as_str()),
                    range.start,
                    range.end,
                    points,
                    q(interp)
                )
            }
        }
        Op::Retake {
            track,
            range,
            dims,
            n_variants,
            seed,
        } => format!(
            "p.track({}).retake({}, {}, [{}], n_variants={n_variants}{})",
            track.0,
            range.start,
            range.end,
            dims.iter()
                .map(|d| q(&format!("{d:?}").to_lowercase()))
                .collect::<Vec<_>>()
                .join(", "),
            seed.map_or(String::new(), |s| format!(", seed={s}"))
        ),
        Op::SelectTake { phrase, take } => format!(
            "p.select_take({}, {})",
            phrase.0,
            take.map_or("None".into(), |t| t.0.to_string())
        ),
        Op::SetTrack { id, patch } if patch.mix.is_some() => {
            let m = patch.mix.as_ref().expect("checked");
            format!(
                "p.track({}).set_mix(gain_db={}, pan={}, mute={}, solo={})",
                id.0,
                m.gain_db,
                m.pan,
                if m.mute { "True" } else { "False" },
                if m.solo { "True" } else { "False" }
            )
        }
        Op::SetSection { name, start, end } => {
            format!("p.set_section({}, {start}, {end})", q(name))
        }
        Op::SetTempo { tick, bpm } => format!("p.set_tempo({tick}, {bpm})"),
        other => format!(
            "p.apply_json({})",
            q(&serde_json::to_string(other).unwrap_or_default())
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cantor_core::Document;
    use std::collections::BTreeSet;

    fn fixture() -> Project {
        let text = include_str!("../../crates/cantor-cli/tests/fixtures/twinkle.cantor");
        cantor_io::project_file::from_bytes(text.as_bytes())
            .unwrap()
            .project
    }

    /// Every registered GUI edit has an Op and a Python equivalent: its
    /// builder produces an Op of the declared kind that applies cleanly,
    /// and the Python method exists in the package's stubs.
    #[test]
    fn every_edit_action_has_an_op_and_a_python_equivalent() {
        let stubs = include_str!("../../crates/cantor-py/python/cantor/__init__.pyi");
        let op_kinds: BTreeSet<String> = {
            let schema = serde_json::to_value(cantor_core::schema::schema_for::<Op>()).unwrap();
            let mut kinds = BTreeSet::new();
            collect_op_tags(&schema, &mut kinds);
            kinds
        };
        let mut p = fixture();
        // Give the phrase two takes so cycling has something to select.
        let mut doc = Document::new(p.clone());
        doc.apply(Op::Retake {
            track: TrackId(1),
            range: TickRange {
                start: 0,
                end: 1920,
            },
            dims: vec![RetakeDim::Pitch],
            n_variants: 2,
            seed: Some(1),
        });
        p = doc.project().clone();
        let ctx = ActionContext {
            selection: vec![NoteId(101), NoteId(102)],
            track: Some(TrackId(1)),
            cursor_tick: 17_500,
            cursor_cents: 6400.0,
            grid: 480,
            text: "ah oh".into(),
            param: Some("breathiness".into()),
            phrase: Some(PhraseId(100)),
        };
        let mut ids = BTreeSet::new();
        for a in REGISTRY {
            assert!(ids.insert(a.id), "duplicate action id {}", a.id);
            if !a.python.is_empty() {
                let (class, method) = a.python.split_once('.').expect("Class.method");
                assert!(
                    stubs.contains(&format!("class {class}"))
                        && (stubs.contains(&format!("def {method}("))
                            || stubs.contains(&format!("    {method}: "))),
                    "{}: Python equivalent {} not in cantor stubs",
                    a.id,
                    a.python
                );
            }
            if a.kind != Kind::Edit {
                continue;
            }
            let tag =
                a.op.unwrap_or_else(|| panic!("{} is an edit without an op", a.id));
            assert!(op_kinds.contains(tag), "{}: unknown op {tag}", a.id);
            assert!(
                !a.python.is_empty(),
                "{}: edit without a Python equivalent",
                a.id
            );
            // Text-driven actions get text they understand.
            let ctx = if a.id == "style.apply" {
                ActionContext {
                    text: "breathier, more vibrato".into(),
                    ..ctx.clone()
                }
            } else {
                ctx.clone()
            };
            let op = (a.build.expect("edits have builders"))(&p, &ctx)
                .unwrap_or_else(|| panic!("{} built no op in the test context", a.id));
            let v = serde_json::to_value(&op).unwrap();
            let built = if v["op"] == "batch" {
                v["ops"][0]["op"].clone()
            } else {
                v["op"].clone()
            };
            assert_eq!(built, tag, "{}", a.id);
            let mut doc = Document::new(p.clone());
            let r = doc.apply(op.clone());
            assert!(r.applied, "{}: {:?}", a.id, r.diagnostics);
            let line = python_line(&op, &p);
            let method = a.python.split_once('.').unwrap().1;
            assert!(line.contains(method), "{}: {line}", a.id);
        }
    }

    fn collect_op_tags(v: &serde_json::Value, out: &mut BTreeSet<String>) {
        match v {
            serde_json::Value::Object(m) => {
                if let Some(serde_json::Value::Object(props)) = m.get("properties")
                    && let Some(c) = props
                        .get("op")
                        .and_then(|o| o.get("const"))
                        .and_then(|c| c.as_str())
                {
                    out.insert(c.to_owned());
                }
                m.values().for_each(|x| collect_op_tags(x, out));
            }
            serde_json::Value::Array(a) => a.iter().for_each(|x| collect_op_tags(x, out)),
            _ => {}
        }
    }

    #[test]
    fn python_lines_read_like_the_api() {
        let p = fixture();
        let line = python_line(
            &Op::SetLyric {
                id: NoteId(100),
                lyric: "la".into(),
            },
            &p,
        );
        assert_eq!(line, "p.note(100).lyric = \"la\"");
    }
}
