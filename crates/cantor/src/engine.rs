//! The engine: one open project with history, voicebanks, renders and the
//! reference vocal. HTTP, MCP and the CLI all call these methods; every
//! edit goes through `cantor_core::Document`.

use cantor_analysis::ReferenceF0;
use cantor_core::{Diagnostic, Document, Op, Project, Revision, TrackId, TuneContext, TuneRequest};
use cantor_io::Kind;
use cantor_synth::{Placement, RenderRequest, Synth, Voicebank, VoicebankInfo};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::mpsc::UnboundedSender;

/// Who made a change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// The HTTP API (the web UI).
    #[default]
    Http,
    /// An agent over MCP.
    Mcp,
    /// The command line.
    Cli,
}

/// The caller of an engine method.
#[derive(Clone, Debug, Default)]
pub struct Caller {
    /// Where the call came from.
    pub source: Source,
    /// The `X-Cantor-Client` id, echoed in events.
    pub client: Option<String>,
}

impl Caller {
    /// An MCP caller.
    pub fn mcp() -> Self {
        Self {
            source: Source::Mcp,
            client: None,
        }
    }
    /// A CLI caller.
    pub fn cli() -> Self {
        Self {
            source: Source::Cli,
            client: None,
        }
    }
}

/// An engine failure.
#[derive(Debug)]
pub struct EngineError {
    /// Stable code: `not_found`, `invalid`, `rejected`, `revision_conflict`, `io`.
    pub code: &'static str,
    /// Explanation.
    pub message: String,
    /// Details.
    pub diagnostics: Vec<Diagnostic>,
}

impl EngineError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            diagnostics: Vec::new(),
        }
    }
    fn rejected(r: cantor_core::Rejected) -> Self {
        let message = r
            .diagnostics
            .first()
            .map_or_else(|| "rejected".to_owned(), |d| d.message.clone());
        Self {
            code: "rejected",
            message,
            diagnostics: r.diagnostics,
        }
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for EngineError {}

/// Result type of engine calls.
pub type Result<T> = std::result::Result<T, EngineError>;

/// `GET /api/state`.
#[derive(Debug, Serialize)]
pub struct StateView {
    /// The project.
    pub project: Project,
    /// Revision.
    pub revision: Revision,
    /// File path, if saved or opened.
    pub path: Option<String>,
    /// Undo available.
    pub can_undo: bool,
    /// Redo available.
    pub can_redo: bool,
    /// Unsaved changes.
    pub dirty: bool,
}

/// One history step.
#[derive(Clone, Debug, Serialize)]
pub struct StepView {
    /// Label.
    pub label: String,
    /// Who made it.
    pub source: Source,
}

/// `GET /api/history`.
#[derive(Debug, Serialize)]
pub struct HistoryView {
    /// Oldest first.
    pub undo: Vec<StepView>,
    /// Next redo first.
    pub redo: Vec<StepView>,
    /// Checkpoint names, oldest first.
    pub checkpoints: Vec<String>,
}

/// The result of an edit.
#[derive(Debug, Serialize)]
pub struct EditResult {
    /// Revision afterwards.
    pub revision: Revision,
    /// Warnings.
    pub diagnostics: Vec<Diagnostic>,
}

/// The result of a tuner pass.
#[derive(Debug, Serialize)]
pub struct TuneResult {
    /// Revision afterwards.
    pub revision: Revision,
    /// Ops applied (0 when the pass changed nothing).
    pub ops_applied: usize,
    /// One line about what happened.
    pub summary: String,
    /// Notes.
    pub diagnostics: Vec<Diagnostic>,
}

/// The result of a render.
#[derive(Debug, Serialize)]
pub struct RenderResult {
    /// Id for `GET /api/render/{id}.wav`.
    pub render_id: String,
    /// The revision rendered.
    pub revision: Revision,
    /// Measurements.
    pub report: cantor_analysis::RenderReport,
    /// The WAV on disk.
    pub wav_path: String,
    /// Problems.
    pub diagnostics: Vec<Diagnostic>,
    /// Phrases synthesized / taken from cache.
    pub phrases_rendered: usize,
    /// Phrases taken from cache.
    pub phrases_cached: usize,
}

struct Reference {
    path: PathBuf,
    /// F0 in audio time (offset not applied).
    f0: ReferenceF0,
}

struct State {
    doc: Document,
    path: Option<PathBuf>,
    undo_src: Vec<Source>,
    redo_src: Vec<Source>,
    renders: VecDeque<(String, PathBuf)>,
    render_seq: u64,
    reference: Option<Reference>,
}

/// The engine.
pub struct Engine {
    state: Mutex<State>,
    banks: RwLock<Vec<Arc<Voicebank>>>,
    bank_paths: Vec<PathBuf>,
    synth: Synth,
    subscribers: Mutex<Vec<UnboundedSender<String>>>,
    render_dir: PathBuf,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine").finish_non_exhaustive()
    }
}

const KEEP_RENDERS: usize = 8;

/// Voicebank search path (docs/design.md "CLI").
pub fn voicebank_paths(extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = extra.to_vec();
    if let Ok(s) = std::env::var("CANTOR_VOICEBANKS") {
        v.extend(std::env::split_paths(&s));
    }
    v.push(PathBuf::from("voicebanks"));
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        v.push(home.join(".local/share/cantor/voicebanks"));
        v.push(home.join("Library/Application Support/Cantor/voicebanks"));
    }
    v
}

fn default_project() -> Project {
    let mut p = Project::default();
    p.tracks.push(cantor_core::Track {
        id: TrackId(1),
        name: "Vocal".into(),
        ..Default::default()
    });
    p.canonicalize();
    p
}

impl Engine {
    /// An engine with an empty project and voicebanks from `bank_paths`.
    pub fn new(bank_paths: Vec<PathBuf>) -> Self {
        let render_dir =
            std::env::temp_dir().join(format!("cantor-renders-{}", std::process::id()));
        let (banks, warnings) = cantor_synth::scan(&bank_paths);
        for w in &warnings {
            tracing::warn!("{w}");
        }
        let mut project = default_project();
        if let Some(b) = banks.first() {
            project.tracks[0].singer = b.id.clone();
        }
        Self {
            state: Mutex::new(State {
                doc: Document::new(project),
                path: None,
                undo_src: Vec::new(),
                redo_src: Vec::new(),
                renders: VecDeque::new(),
                render_seq: 0,
                reference: None,
            }),
            banks: RwLock::new(banks.into_iter().map(Arc::new).collect()),
            bank_paths,
            synth: Synth::new(),
            subscribers: Mutex::new(Vec::new()),
            render_dir,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Rescans the voicebank folders; returns warnings.
    pub fn rescan(&self) -> Vec<String> {
        let (banks, warnings) = cantor_synth::scan(&self.bank_paths);
        for w in &warnings {
            tracing::warn!("{w}");
        }
        *self
            .banks
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            banks.into_iter().map(Arc::new).collect();
        warnings
    }

    fn banks(&self) -> Vec<Arc<Voicebank>> {
        self.banks
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    // ---- events ---------------------------------------------------------------------

    /// Subscribes to events (JSON strings).
    pub fn subscribe(&self, tx: UnboundedSender<String>) {
        self.subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(tx);
    }

    fn emit(&self, event: Value) {
        let s = event.to_string();
        self.subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|tx| tx.send(s.clone()).is_ok());
    }

    fn revision_event(&self, revision: Revision, label: &str, c: &Caller) {
        self.emit(json!({ "type": "revision", "revision": revision, "label": label, "source": c.source, "client": c.client }));
    }

    // ---- reading ----------------------------------------------------------------------

    /// The project and its status.
    pub fn state(&self) -> StateView {
        let st = self.lock();
        StateView {
            project: st.doc.project().clone(),
            revision: st.doc.revision(),
            path: st.path.as_ref().map(|p| p.display().to_string()),
            can_undo: st.doc.can_undo(),
            can_redo: st.doc.can_redo(),
            dirty: st.doc.is_dirty(),
        }
    }

    /// A copy of the project.
    pub fn project(&self) -> Project {
        self.lock().doc.project().clone()
    }

    /// Undo/redo steps and checkpoints.
    pub fn history(&self) -> HistoryView {
        let st = self.lock();
        let undo = st
            .doc
            .undo_labels()
            .zip(st.undo_src.iter())
            .map(|(l, s)| StepView {
                label: l.to_owned(),
                source: *s,
            })
            .collect();
        let mut redo: Vec<StepView> = st
            .doc
            .redo_labels()
            .zip(st.redo_src.iter())
            .map(|(l, s)| StepView {
                label: l.to_owned(),
                source: *s,
            })
            .collect();
        redo.reverse();
        HistoryView {
            undo,
            redo,
            checkpoints: st.doc.checkpoints().map(str::to_owned).collect(),
        }
    }

    /// Installed voicebanks.
    pub fn voicebanks(&self) -> Vec<VoicebankInfo> {
        self.banks().iter().map(|b| b.info()).collect()
    }

    /// A voicebank's aliases and details.
    pub fn voicebank_info(&self, id: &str) -> Result<Value> {
        let b = self
            .banks()
            .into_iter()
            .find(|b| b.id == id)
            .ok_or_else(|| EngineError::new("not_found", format!("no voicebank `{id}`")))?;
        Ok(
            json!({ "info": b.info(), "aliases": b.entries.keys().collect::<Vec<_>>(), "warnings": b.warnings,
            "prefix_map": b.prefix_map.iter().map(|(k, (p, s))| json!({ "key": k, "prefix": p, "suffix": s })).collect::<Vec<_>>() }),
        )
    }

    /// The voicebank's portrait.
    pub fn voicebank_image(&self, id: &str) -> Result<(Vec<u8>, &'static str)> {
        let b = self
            .banks()
            .into_iter()
            .find(|b| b.id == id)
            .ok_or_else(|| EngineError::new("not_found", format!("no voicebank `{id}`")))?;
        let p = b
            .image
            .clone()
            .ok_or_else(|| EngineError::new("not_found", "the voicebank has no image"))?;
        let bytes = std::fs::read(&p).map_err(|e| EngineError::new("io", e.to_string()))?;
        let mime = match p
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("png") => "image/png",
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("gif") => "image/gif",
            _ => "image/bmp",
        };
        Ok((bytes, mime))
    }

    fn track_of(p: &Project, id: TrackId) -> Result<&cantor_core::Track> {
        p.track(id)
            .ok_or_else(|| EngineError::new("not_found", format!("no track {}", id.0)))
    }

    /// The final pitch curve, in the UI's wire form.
    pub fn pitch(&self, track: TrackId) -> Result<Value> {
        let p = self.project();
        let t = Self::track_of(&p, track)?;
        let c = cantor_core::render_pitch(t, &p.tempo_map(), None);
        let cents: Vec<Option<f32>> = c
            .cents
            .iter()
            .zip(&c.voiced)
            .map(|(v, on)| on.then_some(*v))
            .collect();
        Ok(
            json!({ "start_ms": c.start_sec * 1000.0, "step_ms": c.step_sec * 1000.0, "cents": cents }),
        )
    }

    /// Where each note's sample lands.
    pub fn phonemes(&self, track: TrackId) -> Result<Vec<Placement>> {
        let p = self.project();
        let t = Self::track_of(&p, track)?;
        Ok(cantor_synth::phonemes(&p, t, &self.banks()))
    }

    /// Notes of a track in a compact table form, optionally limited to a tick range.
    pub fn notes(&self, track: Option<TrackId>, range: Option<(i64, i64)>) -> Result<Value> {
        let p = self.project();
        let tempo = p.tempo_map();
        let tracks: Vec<&cantor_core::Track> = match track {
            Some(id) => vec![Self::track_of(&p, id)?],
            None => p.tracks.iter().collect(),
        };
        Ok(json!(tracks.iter().map(|t| json!({
            "track": t.id, "name": t.name, "singer": t.singer,
            "columns": ["id", "start", "length", "key", "lyric", "sec"],
            "notes": t.notes.iter().filter(|n| range.is_none_or(|(a, b)| n.start < b && n.end() > a))
                .map(|n| json!([n.id, n.start, n.length, n.key, n.lyric, (tempo.tick_to_sec(n.start) * 1000.0).round() / 1000.0]))
                .collect::<Vec<_>>(),
        })).collect::<Vec<_>>()))
    }

    // ---- editing ------------------------------------------------------------------------

    fn commit(&self, st: &mut State, op: Op, c: &Caller) -> Result<cantor_core::Applied> {
        let applied = st.doc.apply(op).map_err(EngineError::rejected)?;
        st.undo_src.push(c.source);
        st.redo_src.clear();
        Ok(applied)
    }

    /// Applies ops as one undo step. A stale `base_revision` is rejected.
    pub fn apply_ops(
        &self,
        ops: Vec<Op>,
        base: Option<Revision>,
        c: &Caller,
    ) -> Result<EditResult> {
        let mut st = self.lock();
        let cur = st.doc.revision();
        if let Some(b) = base.filter(|b| *b != cur) {
            return Err(EngineError::new(
                "revision_conflict",
                format!("the project is at revision {}, not {}", cur.0, b.0),
            ));
        }
        let op = match ops.len() {
            0 => {
                return Ok(EditResult {
                    revision: cur,
                    diagnostics: Vec::new(),
                });
            }
            1 => ops.into_iter().next().expect("one op"),
            n => Op::batch(format!("{n} edits"), ops),
        };
        let a = self.commit(&mut st, op, c)?;
        drop(st);
        self.revision_event(a.revision, &a.label, c);
        Ok(EditResult {
            revision: a.revision,
            diagnostics: a.diagnostics,
        })
    }

    /// Undoes (`redo = false`) or redoes up to `steps` steps.
    pub fn undo_redo(&self, redo: bool, steps: usize, c: &Caller) -> Result<EditResult> {
        let mut st = self.lock();
        let mut last = None;
        for _ in 0..steps.max(1) {
            let r =
                if redo { st.doc.redo() } else { st.doc.undo() }.map_err(EngineError::rejected)?;
            let Some(a) = r else { break };
            // Sources move between the stacks with their steps.
            if redo {
                let s = st.redo_src.pop().unwrap_or_default();
                st.undo_src.push(s);
            } else {
                let s = st.undo_src.pop().unwrap_or_default();
                st.redo_src.push(s);
            }
            last = Some(a);
        }
        let a = last.ok_or_else(|| {
            EngineError::new(
                "empty",
                if redo {
                    "nothing to redo"
                } else {
                    "nothing to undo"
                },
            )
        })?;
        drop(st);
        self.revision_event(
            a.revision,
            &format!("{} {}", if redo { "Redo" } else { "Undo" }, a.label),
            c,
        );
        Ok(EditResult {
            revision: a.revision,
            diagnostics: a.diagnostics,
        })
    }

    /// Saves a named snapshot.
    pub fn checkpoint(&self, name: &str, c: &Caller) -> Result<HistoryView> {
        let name = name.trim();
        if name.is_empty() {
            return Err(EngineError::new("invalid", "a checkpoint needs a name"));
        }
        self.lock().doc.checkpoint(name);
        self.emit(
            json!({ "type": "checkpoint", "name": name, "source": c.source, "client": c.client }),
        );
        Ok(self.history())
    }

    /// Returns to a checkpoint as one undoable step.
    pub fn restore(&self, name: &str, c: &Caller) -> Result<EditResult> {
        let mut st = self.lock();
        let a = st.doc.restore(name).map_err(EngineError::rejected)?;
        st.undo_src.push(c.source);
        st.redo_src.clear();
        drop(st);
        self.revision_event(a.revision, &a.label, c);
        Ok(EditResult {
            revision: a.revision,
            diagnostics: a.diagnostics,
        })
    }

    /// Runs a tuner pass.
    pub fn tune(&self, req: TuneRequest, c: &Caller) -> Result<TuneResult> {
        let ctx = TuneContext {
            reference_f0: self.reference_for_tune()?,
        };
        let mut st = self.lock();
        let r =
            cantor_core::tune::run(st.doc.project(), &req, &ctx).map_err(EngineError::rejected)?;
        let n: usize = r
            .ops
            .iter()
            .map(|o| match o {
                Op::Batch { ops, .. } => ops.len(),
                _ => 1,
            })
            .sum();
        if r.ops.is_empty() {
            return Ok(TuneResult {
                revision: st.doc.revision(),
                ops_applied: 0,
                summary: r.summary,
                diagnostics: r.diagnostics,
            });
        }
        let op = if r.ops.len() == 1 {
            r.ops.into_iter().next().expect("one")
        } else {
            Op::batch(r.summary.clone(), r.ops)
        };
        let a = self.commit(&mut st, op, c)?;
        drop(st);
        self.revision_event(a.revision, &a.label, c);
        let mut diagnostics = r.diagnostics;
        diagnostics.extend(a.diagnostics);
        Ok(TuneResult {
            revision: a.revision,
            ops_applied: n,
            summary: r.summary,
            diagnostics,
        })
    }

    fn replace(
        &self,
        project: Project,
        label: &str,
        path: Option<PathBuf>,
        fresh: bool,
        c: &Caller,
    ) -> Result<EditResult> {
        let mut st = self.lock();
        let a = if fresh {
            st.doc = Document::new(project);
            st.undo_src.clear();
            st.redo_src.clear();
            st.doc.mark_saved();
            cantor_core::Applied {
                revision: st.doc.revision(),
                label: label.to_owned(),
                diagnostics: Vec::new(),
            }
        } else {
            let a = st
                .doc
                .replace(project, label)
                .map_err(EngineError::rejected)?;
            st.undo_src.push(c.source);
            st.redo_src.clear();
            a
        };
        if fresh {
            st.path = path;
            st.reference = None;
        }
        drop(st);
        self.revision_event(a.revision, label, c);
        Ok(EditResult {
            revision: a.revision,
            diagnostics: a.diagnostics,
        })
    }

    /// Starts an empty project.
    pub fn new_project(&self, c: &Caller) -> Result<StateView> {
        let mut p = default_project();
        if let Some(b) = self.banks().first() {
            p.tracks[0].singer = b.id.clone();
        }
        self.replace(p, "New project", None, true, c)?;
        Ok(self.state())
    }

    /// Opens a `.cantor` file.
    pub fn open(&self, path: &Path, c: &Caller) -> Result<EditResult> {
        let l = cantor_io::project_file::load(path)
            .map_err(|e| EngineError::new("io", e.to_string()))?;
        // A converted v1 project has no save path, so Save never overwrites the original.
        let keep = (!l.from_v1).then(|| path.to_owned());
        let mut r = self.replace(l.project, "Open project", keep, true, c)?;
        r.diagnostics.extend(l.diagnostics);
        Ok(r)
    }

    /// Saves to `path` (or the current path).
    pub fn save(&self, path: Option<&Path>) -> Result<String> {
        let mut st = self.lock();
        let path = path
            .map(Path::to_owned)
            .or_else(|| st.path.clone())
            .ok_or_else(|| EngineError::new("invalid", "no path to save to"))?;
        cantor_io::project_file::save(&path, st.doc.project())
            .map_err(|e| EngineError::new("io", format!("{}: {e}", path.display())))?;
        st.doc.mark_saved();
        st.path = Some(path.clone());
        Ok(path.display().to_string())
    }

    /// Imports a file. Audio becomes the reference; lyrics text fills the
    /// first track's notes; scores replace the project (undoably).
    pub fn import(
        &self,
        path: &Path,
        kind: Option<&str>,
        bytes: Option<Vec<u8>>,
        c: &Caller,
    ) -> Result<EditResult> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        let kind = kind
            .map(str::to_owned)
            .unwrap_or_else(|| match ext.as_str() {
                "wav" | "flac" | "mp3" | "ogg" | "aif" | "aiff" => "audio".into(),
                "txt" => "lyrics".into(),
                _ => String::new(),
            });
        let read = || match &bytes {
            Some(b) => Ok(b.clone()),
            None => std::fs::read(path)
                .map_err(|e| EngineError::new("io", format!("{}: {e}", path.display()))),
        };
        match kind.as_str() {
            "audio" => {
                let path = match &bytes {
                    // Uploaded audio is kept next to the renders for this session.
                    Some(b) => {
                        let dir = self.render_dir.join("uploads");
                        std::fs::create_dir_all(&dir)
                            .map_err(|e| EngineError::new("io", e.to_string()))?;
                        let p = dir.join(path.file_name().unwrap_or_default());
                        std::fs::write(&p, b).map_err(|e| EngineError::new("io", e.to_string()))?;
                        p
                    }
                    None => path
                        .canonicalize()
                        .map_err(|e| EngineError::new("io", format!("{}: {e}", path.display())))?,
                };
                self.analyze_reference_file(&path)?;
                let r = cantor_core::Reference {
                    path: path.display().to_string(),
                    offset_ms: 0.0,
                    gain_db: 0.0,
                    muted: false,
                };
                self.apply_ops(vec![Op::SetReference { reference: Some(r) }], None, c)
            }
            "lyrics" => {
                let text = String::from_utf8_lossy(&read()?).into_owned();
                let syl = cantor_io::lyrics::parse(&text, Default::default());
                let p = self.project();
                let t = p
                    .tracks
                    .first()
                    .ok_or_else(|| EngineError::new("invalid", "the project has no track"))?;
                let (op, mut diags) = cantor_io::lyrics::assign(t, &syl, None);
                let mut r = self.apply_ops(vec![Op::batch("Import lyrics", vec![op])], None, c)?;
                diags.append(&mut r.diagnostics);
                r.diagnostics = diags;
                Ok(r)
            }
            k => {
                let kind = match k {
                    "" => Kind::from_path(path).ok_or_else(|| {
                        EngineError::new(
                            "invalid",
                            format!("cannot tell the format of {}", path.display()),
                        )
                    })?,
                    k => serde_json::from_value(json!(k)).map_err(|_| {
                        EngineError::new("invalid", format!("unknown import kind `{k}`"))
                    })?,
                };
                let imp = cantor_io::import_bytes(&read()?, kind, &Default::default())
                    .map_err(|e| EngineError::new("invalid", e.to_string()))?;
                let mut project = imp.project;
                // Give singer-less tracks the first voicebank.
                if let Some(b) = self.banks().first() {
                    for t in project.tracks.iter_mut().filter(|t| t.singer.is_empty()) {
                        t.singer = b.id.clone();
                    }
                }
                let name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let mut r = self.replace(project, &format!("Import {name}"), None, false, c)?;
                r.diagnostics.extend(imp.diagnostics);
                Ok(r)
            }
        }
    }

    /// Exports the project: (bytes, file extension, MIME type, diagnostics).
    pub fn export(
        &self,
        format: &str,
    ) -> Result<(Vec<u8>, &'static str, &'static str, Vec<Diagnostic>)> {
        let p = self.project();
        let map = |e: cantor_io::ExportError| EngineError::new("invalid", e.to_string());
        Ok(match format {
            "wav" => {
                let r = self.render(None, &Caller::default())?;
                (
                    std::fs::read(&r.wav_path)
                        .map_err(|e| EngineError::new("io", e.to_string()))?,
                    "wav",
                    "audio/wav",
                    r.diagnostics,
                )
            }
            "ust" => {
                let e = cantor_io::ust::export(&p, None).map_err(map)?;
                (e.bytes, "ust", "text/plain", e.diagnostics)
            }
            "ustx" => {
                let e = cantor_io::ustx::export(&p).map_err(map)?;
                (e.bytes, "ustx", "text/yaml", e.diagnostics)
            }
            "cantor" => (
                cantor_io::project_file::to_bytes(&p),
                "cantor",
                "application/json",
                Vec::new(),
            ),
            f => {
                return Err(EngineError::new(
                    "invalid",
                    format!("unknown export format `{f}` (wav, ust, ustx, cantor)"),
                ));
            }
        })
    }

    // ---- rendering & reference --------------------------------------------------------------

    fn analyze_reference_file(&self, path: &Path) -> Result<ReferenceF0> {
        {
            let st = self.lock();
            if let Some(r) = st.reference.as_ref().filter(|r| r.path == path) {
                return Ok(r.f0.clone());
            }
        }
        let (samples, sr) = cantor_synth::psola::read_mono(path)
            .map_err(|e| EngineError::new("io", format!("{e} (reference audio must be WAV)")))?;
        let f0 = cantor_analysis::analyze_reference(&samples, sr);
        self.lock().reference = Some(Reference {
            path: path.to_owned(),
            f0: f0.clone(),
        });
        Ok(f0)
    }

    /// The reference audio file: absolute, or relative to the project file.
    fn reference_path(&self) -> Option<(PathBuf, cantor_core::Reference)> {
        let st = self.lock();
        let r = st.doc.project().reference.clone()?;
        let p = PathBuf::from(&r.path);
        let p = match (&st.path, p.is_relative()) {
            (Some(proj), true) => proj.parent().unwrap_or(Path::new(".")).join(p),
            _ => p,
        };
        Some((p, r))
    }

    /// The reference F0 in audio time, if the project has a reference.
    fn reference_f0(&self) -> Result<Option<(ReferenceF0, f64)>> {
        let Some((path, r)) = self.reference_path() else {
            return Ok(None);
        };
        let f0 = self.analyze_reference_file(&path)?;
        Ok(Some((f0, r.offset_ms / 1000.0)))
    }

    fn reference_for_tune(&self) -> Result<Option<cantor_core::tune::ReferenceF0>> {
        Ok(self
            .reference_f0()?
            .map(|(f, off)| cantor_core::tune::ReferenceF0 {
                start_sec: f.start_sec - off,
                step_sec: f.step_sec,
                cents: f.cents,
            }))
    }

    /// `GET /api/reference/f0`: F0 in audio time (the UI applies the offset).
    pub fn reference_f0_view(&self) -> Result<Value> {
        let (f, _) = self
            .reference_f0()?
            .ok_or_else(|| EngineError::new("not_found", "the project has no reference"))?;
        Ok(
            json!({ "start_ms": f.start_sec * 1000.0, "step_ms": f.step_sec * 1000.0, "cents": f.cents }),
        )
    }

    /// The reference audio file.
    pub fn reference_audio(&self) -> Result<Vec<u8>> {
        let (p, _) = self
            .reference_path()
            .ok_or_else(|| EngineError::new("not_found", "the project has no reference"))?;
        std::fs::read(&p).map_err(|e| EngineError::new("io", format!("{}: {e}", p.display())))
    }

    /// Transcribes the reference vocal into notes (no lyrics) on a new track.
    pub fn transcribe(&self, c: &Caller) -> Result<EditResult> {
        let (f, off) = self
            .reference_f0()?
            .ok_or_else(|| EngineError::new("not_found", "the project has no reference"))?;
        let p = self.project();
        let notes =
            cantor_analysis::transcribe(&f.shifted(-off), &p.tempo_map(), &Default::default());
        let singer = self
            .banks()
            .first()
            .map(|b| b.id.clone())
            .unwrap_or_default();
        let track = cantor_core::Track {
            name: "Transcribed".into(),
            singer,
            notes,
            ..Default::default()
        };
        self.apply_ops(vec![Op::AddTrack { track, index: None }], None, c)
    }

    /// Renders the project, keeps the WAV and reports.
    pub fn render(&self, tracks: Option<Vec<TrackId>>, c: &Caller) -> Result<RenderResult> {
        let (project, revision) = {
            let st = self.lock();
            (st.doc.project().clone(), st.doc.revision())
        };
        self.emit(json!({ "type": "progress", "message": "Rendering…" }));
        let reference = self
            .reference_f0()
            .ok()
            .flatten()
            .map(|(f, off)| f.shifted(-off));
        let banks = self.banks();
        let out = self.synth.render(&RenderRequest {
            project: &project,
            banks: &banks,
            reference: reference.as_ref(),
            tracks: tracks.as_deref(),
        });
        let mut st = self.lock();
        st.render_seq += 1;
        let id = format!("r{}-{}", revision.0, st.render_seq);
        std::fs::create_dir_all(&self.render_dir)
            .map_err(|e| EngineError::new("io", e.to_string()))?;
        let path = self.render_dir.join(format!("{id}.wav"));
        std::fs::write(&path, cantor_synth::wav_bytes(&out.samples, 2))
            .map_err(|e| EngineError::new("io", e.to_string()))?;
        st.renders.push_back((id.clone(), path.clone()));
        while st.renders.len() > KEEP_RENDERS {
            if let Some((_, old)) = st.renders.pop_front() {
                let _ = std::fs::remove_file(old);
            }
        }
        drop(st);
        let result = RenderResult {
            render_id: id,
            revision,
            report: out.report,
            wav_path: path.display().to_string(),
            diagnostics: out.diagnostics,
            phrases_rendered: out.phrases_rendered,
            phrases_cached: out.phrases_cached,
        };
        self.emit(json!({ "type": "progress", "message": null }));
        self.emit(json!({ "type": "render", "revision": revision, "report": result.report, "source": c.source, "client": c.client }));
        Ok(result)
    }

    /// A kept render's WAV.
    pub fn render_wav(&self, id: &str) -> Result<Vec<u8>> {
        let p = self
            .lock()
            .renders
            .iter()
            .find(|(i, _)| i == id)
            .map(|(_, p)| p.clone());
        let p = p.ok_or_else(|| {
            EngineError::new(
                "not_found",
                format!("no render `{id}` (only the last {KEEP_RENDERS} are kept)"),
            )
        })?;
        std::fs::read(p).map_err(|e| EngineError::new("io", e.to_string()))
    }

    /// Params, ops and effects schemas.
    pub fn schema(&self) -> Value {
        json!({
            "params": cantor_core::params::curves().iter().chain(cantor_core::params::note_params()).collect::<Vec<_>>(),
            "ops": cantor_core::schema::schema_for::<Op>(),
            "effects": cantor_core::effects::registry(),
            "tune": cantor_core::schema::schema_for::<TuneRequest>(),
        })
    }
}
