//! The editor model: viewport, selection, and edits through the engine.
//! Independent of GPUI so it can be tested and benchmarked headlessly.

use crate::actions::{self, ActionContext, Kind};
use cantor_core::{ApplyResult, NoteId, Op, PhraseId, Project, Revision, TrackId};
use cantor_server::engine::{HistoryEntry, RenderSummary};
use cantor_server::{Engine, EngineError};
use std::collections::BTreeSet;
use std::sync::Arc;

/// Maps ticks and cents to pixels in the piano roll.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// Tick at the left edge.
    pub tick0: f64,
    /// Pitch (cents) at the top edge.
    pub cents_top: f64,
    /// Horizontal zoom.
    pub px_per_tick: f64,
    /// Vertical zoom.
    pub px_per_cent: f64,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            tick0: 0.0,
            cents_top: 8400.0,
            px_per_tick: 0.06,
            px_per_cent: 0.18,
        }
    }
}

impl Viewport {
    /// X position of a tick.
    pub fn x(&self, tick: f64) -> f64 {
        (tick - self.tick0) * self.px_per_tick
    }
    /// Y position of a pitch.
    pub fn y(&self, cents: f64) -> f64 {
        (self.cents_top - cents) * self.px_per_cent
    }
    /// Tick at an x position.
    pub fn tick(&self, x: f64) -> f64 {
        self.tick0 + x / self.px_per_tick
    }
    /// Pitch at a y position.
    pub fn cents(&self, y: f64) -> f64 {
        self.cents_top - y / self.px_per_cent
    }
    /// Zooms horizontally around x by `factor`.
    pub fn zoom_x(&mut self, factor: f64, x: f64) {
        let t = self.tick(x);
        self.px_per_tick = (self.px_per_tick * factor).clamp(0.005, 2.0);
        self.tick0 = t - x / self.px_per_tick;
    }
    /// Scrolls by pixels.
    pub fn scroll(&mut self, dx: f64, dy: f64) {
        self.tick0 = (self.tick0 + dx / self.px_per_tick).max(-1920.0);
        self.cents_top = (self.cents_top - dy / self.px_per_cent).clamp(2400.0, 12_000.0);
    }
}

/// One Script Log line.
#[derive(Clone, Debug, PartialEq)]
pub struct LogEntry {
    /// History label.
    pub label: String,
    /// The equivalent Python.
    pub python: String,
    /// The op as JSON.
    pub op_json: String,
}

/// The editor state for one open project.
pub struct Editor {
    engine: Arc<Engine>,
    project_id: String,
    snapshot: Arc<Project>,
    revision: Revision,
    /// Selected notes.
    pub selection: BTreeSet<NoteId>,
    /// The track keyboard edits apply to.
    pub track: TrackId,
    /// Piano-roll view.
    pub viewport: Viewport,
    /// Grid step in ticks.
    pub grid: i64,
    /// Cursor (absolute tick, cents).
    pub cursor: (i64, f64),
    /// Applied edits as Python, newest last.
    pub script_log: Vec<LogEntry>,
    /// Python lines being recorded as a macro.
    pub recording: Option<Vec<String>>,
    /// The last render and its analysis.
    pub last_render: Option<RenderSummary>,
    /// One-line status.
    pub status: String,
    /// Parameter shown in the parameter lane.
    pub param: String,
}

impl std::fmt::Debug for Editor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Editor")
            .field("project", &self.project_id)
            .finish_non_exhaustive()
    }
}

impl Editor {
    /// Opens a project in the engine.
    pub fn new(engine: Arc<Engine>, project: Project) -> Self {
        let info = engine.add_project(project);
        let (snapshot, revision) = engine.snapshot(&info.project_id).expect("just added");
        let track = snapshot.tracks.first().map_or(TrackId(1), |t| t.id);
        Self {
            engine,
            project_id: info.project_id,
            snapshot,
            revision,
            selection: BTreeSet::new(),
            track,
            viewport: Viewport::default(),
            grid: 480,
            cursor: (0, 6000.0),
            script_log: Vec::new(),
            recording: None,
            last_render: None,
            status: String::new(),
            param: "breathiness".into(),
        }
    }

    /// The engine.
    pub fn engine(&self) -> &Arc<Engine> {
        &self.engine
    }

    /// The engine's project handle.
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// The current project.
    pub fn project(&self) -> &Arc<Project> {
        &self.snapshot
    }

    /// The current revision.
    pub fn revision(&self) -> Revision {
        self.revision
    }

    fn refresh(&mut self) {
        if let Ok((s, r)) = self.engine.snapshot(&self.project_id) {
            self.snapshot = s;
            self.revision = r;
        }
        self.selection
            .retain(|id| self.snapshot.note(*id).is_some());
    }

    /// Applies an op, logging its Python equivalent. Rejections become the
    /// status line (never silent).
    pub fn apply(&mut self, op: Op) -> Result<ApplyResult, EngineError> {
        let before = self.snapshot.clone();
        let python = actions::python_line(&op, &before);
        let label = op.label();
        let op_json = serde_json::to_string(&op).unwrap_or_default();
        let r = self
            .engine
            .apply_ops(&self.project_id, self.revision, vec![op], false)?;
        if r.applied {
            if let Some(rec) = &mut self.recording {
                rec.push(python.clone());
            }
            self.script_log.push(LogEntry {
                label,
                python,
                op_json,
            });
            self.status.clear();
        } else {
            self.status = r
                .diagnostics
                .iter()
                .map(|d| d.message.clone())
                .collect::<Vec<_>>()
                .join("; ");
        }
        for w in r.diagnostics.iter().filter(|d| !d.is_error()) {
            self.status = w.message.clone();
        }
        self.refresh();
        Ok(r)
    }

    /// The context actions are built against.
    pub fn context(&self) -> ActionContext {
        let mut selection: Vec<NoteId> = self.selection.iter().copied().collect();
        selection.sort_by_key(|id| {
            self.snapshot
                .note(*id)
                .map_or(i64::MAX, |(_, p, n)| p.start + n.start)
        });
        let phrase = self.snapshot.track(self.track).and_then(|t| {
            cantor_core::phrase_spans(&self.snapshot, t)
                .into_iter()
                .find(|s| self.cursor.0 >= s.start && self.cursor.0 < s.end)
                .map(|s| s.id)
        });
        ActionContext {
            selection,
            track: Some(self.track),
            cursor_tick: self.cursor.0,
            cursor_cents: self.cursor.1,
            grid: self.grid,
            text: String::new(),
            param: Some(self.param.clone()),
            phrase,
        }
    }

    /// Runs a registered action. Returns false if it does not apply here.
    pub fn run(&mut self, id: &str, text: Option<&str>) -> bool {
        let Some(def) = actions::find(id) else {
            return false;
        };
        match def.kind {
            Kind::Edit => {
                let mut ctx = self.context();
                if let Some(t) = text {
                    ctx.text = t.to_owned();
                }
                match (def.build.expect("edits build ops"))(&self.snapshot, &ctx) {
                    Some(op) => {
                        let added = match &op {
                            Op::AddNote { note, .. } => Some(note.id),
                            _ => None,
                        };
                        let ok = self.apply(op).is_ok_and(|r| r.applied);
                        if let (true, Some(id)) = (ok, added) {
                            self.selection = BTreeSet::from([id]);
                        }
                        ok
                    }
                    None => {
                        self.status = format!("{}: nothing to apply it to", def.name);
                        false
                    }
                }
            }
            Kind::History => match id {
                "history.undo" => self.undo(),
                "history.redo" => self.redo(),
                "history.checkpoint" => {
                    let name = text
                        .map_or_else(|| format!("checkpoint {}", self.revision.0), str::to_owned);
                    let ok = self.engine.checkpoint(&self.project_id, &name).is_ok();
                    self.log_python(&format!("p.checkpoint({name:?})"));
                    ok
                }
                _ => false,
            },
            Kind::View => match id {
                "view.zoom_in" => {
                    self.viewport.zoom_x(1.25, 0.0);
                    true
                }
                "view.zoom_out" => {
                    self.viewport.zoom_x(0.8, 0.0);
                    true
                }
                "view.select_all" => {
                    self.selection = self
                        .snapshot
                        .track(self.track)
                        .map(|t| {
                            t.parts
                                .iter()
                                .flat_map(|p| p.notes.iter().map(|n| n.id))
                                .collect()
                        })
                        .unwrap_or_default();
                    true
                }
                "macro.toggle" => {
                    self.toggle_recording();
                    true
                }
                _ => false,
            },
        }
    }

    fn log_python(&mut self, line: &str) {
        if let Some(rec) = &mut self.recording {
            rec.push(line.to_owned());
        }
        self.script_log.push(LogEntry {
            label: line.to_owned(),
            python: line.to_owned(),
            op_json: String::new(),
        });
    }

    /// Undo.
    pub fn undo(&mut self) -> bool {
        let ok = self.engine.undo(&self.project_id).ok().flatten().is_some();
        if ok {
            self.log_python("p.undo()");
        }
        self.refresh();
        ok
    }

    /// Redo.
    pub fn redo(&mut self) -> bool {
        let ok = self.engine.redo(&self.project_id).ok().flatten().is_some();
        if ok {
            self.log_python("p.redo()");
        }
        self.refresh();
        ok
    }

    /// Returns to a history node or checkpoint (`#<node>` or a name).
    pub fn restore(&mut self, name: &str) -> bool {
        let ok = self
            .engine
            .restore(&self.project_id, self.revision, name)
            .is_ok();
        if ok {
            self.log_python(&format!("p.restore({name:?})"));
        }
        self.refresh();
        ok
    }

    /// The undo tree.
    pub fn history(&self) -> Vec<HistoryEntry> {
        self.engine
            .list_history(&self.project_id)
            .unwrap_or_default()
    }

    /// Starts recording, or stops and returns the macro as a Python script.
    pub fn toggle_recording(&mut self) -> Option<String> {
        match self.recording.take() {
            Some(lines) => {
                let script = macro_script(&lines);
                self.status = format!("macro recorded: {} steps", lines.len());
                Some(script)
            }
            None => {
                self.recording = Some(Vec::new());
                self.status = "recording macro".into();
                None
            }
        }
    }

    /// The note under a piano-roll position, if any.
    pub fn note_at(&self, x: f64, y: f64) -> Option<NoteId> {
        let tick = self.viewport.tick(x);
        let cents = self.viewport.cents(y);
        let track = self.snapshot.track(self.track)?;
        track.parts.iter().find_map(|p| {
            p.notes.iter().rev().find_map(|n| {
                let s = (p.start + n.start) as f64;
                let inside = tick >= s
                    && tick < s + n.length as f64
                    && (cents - n.pitch_cents).abs() <= 50.0;
                inside.then_some(n.id)
            })
        })
    }

    /// The phrase under the cursor.
    pub fn phrase_at_cursor(&self) -> Option<PhraseId> {
        self.context().phrase
    }
}

/// A recorded macro as an editable Python script.
pub fn macro_script(lines: &[String]) -> String {
    let mut s = String::from(
        "# Cantor macro: replay with `python macro.py song.cantor`.\nimport sys\n\nimport cantor as ct\n\np = ct.open(sys.argv[1])\nwith p.transaction(\"macro\"):\n",
    );
    if lines.is_empty() {
        s.push_str("    pass\n");
    }
    for l in lines {
        for part in l.lines() {
            s.push_str("    ");
            s.push_str(part);
            s.push('\n');
        }
    }
    s.push_str("p.save()\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use cantor_server::EngineConfig;

    fn editor() -> Editor {
        let tmp = std::env::temp_dir().join(format!("cantor-app-{}", std::process::id()));
        let engine = Arc::new(Engine::new(EngineConfig {
            singer_paths: vec![],
            vocoder_paths: vec![],
            rmvpe: None,
            cache_dir: tmp.join("c"),
            render_dir: tmp.join("r"),
        }));
        Editor::new(engine, crate::demo::project(40))
    }

    #[test]
    fn viewport_round_trips() {
        let mut v = Viewport::default();
        v.zoom_x(1.7, 300.0);
        v.scroll(120.0, -40.0);
        assert!((v.tick(v.x(5000.0)) - 5000.0).abs() < 1e-9);
        assert!((v.cents(v.y(6100.0)) - 6100.0).abs() < 1e-9);
    }

    #[test]
    fn actions_log_python_and_undo() {
        let mut e = editor();
        let first = e.project().notes().next().unwrap().2.id;
        e.selection.insert(first);
        assert!(e.run("note.up", None));
        assert_eq!(
            e.script_log.last().unwrap().python,
            format!("p.note({}).move_by(dt=0, dcents=100)", first.0)
        );
        assert_eq!(e.revision(), Revision(1));
        assert!(e.run("history.undo", None));
        assert_eq!(e.script_log.last().unwrap().python, "p.undo()");
        e.run("macro.toggle", None);
        e.run("expr.vibrato", None);
        let script = e.toggle_recording().unwrap();
        assert!(
            script.contains(".vibrato(") && script.contains("import cantor as ct"),
            "{script}"
        );
    }

    #[test]
    fn hit_testing_finds_notes() {
        let e = editor();
        let (_, p, n) = e.project().notes().next().unwrap();
        let x = e.viewport.x((p.start + n.start) as f64 + 10.0);
        let y = e.viewport.y(n.pitch_cents);
        assert_eq!(e.note_at(x, y), Some(n.id));
        assert_eq!(e.note_at(x, y + 500.0), None);
    }
}
