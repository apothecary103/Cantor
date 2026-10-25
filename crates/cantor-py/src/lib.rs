//! Python bindings: `import cantor as ct`.
//!
//! Handles (`Project`, `Track`, `Note`) hold ids, never document data; every
//! read takes a fresh snapshot and every write is an `Op` applied through
//! the engine (or collected by a transaction and applied as one batch).

// pyo3's macros expand to code this lint flags.
#![allow(unsafe_code)]

use cantor_core::{
    Interp, NoteExprPatch, NoteId, Op, ParamId, Project as Doc, TickRange, TrackId, Vibrato,
    phrase_spans,
};
use cantor_render::render::Quality;
use cantor_server::engine::{RenderSummary, TrackRef, resolve_track};
use cantor_server::{Engine, EngineConfig, EngineError};
use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyType;
use std::sync::{Arc, Mutex, OnceLock};

create_exception!(
    cantor,
    CantorError,
    PyException,
    "An error reported by the Cantor engine."
);

fn engine() -> Arc<Engine> {
    static E: OnceLock<Arc<Engine>> = OnceLock::new();
    E.get_or_init(|| Arc::new(Engine::new(EngineConfig::from_env())))
        .clone()
}

fn err(e: EngineError) -> PyErr {
    CantorError::new_err(e.to_string())
}

/// An open project.
#[pyclass(module = "cantor")]
struct Project {
    engine: Arc<Engine>,
    id: String,
    /// Ops collected by an open transaction.
    txn: Mutex<Option<(String, Vec<Op>)>>,
}

impl Project {
    fn doc(&self) -> PyResult<Arc<Doc>> {
        Ok(self.engine.snapshot(&self.id).map_err(err)?.0)
    }

    fn submit(&self, op: Op) -> PyResult<()> {
        if let Some((_, ops)) = self.txn.lock().expect("txn lock").as_mut() {
            ops.push(op);
            return Ok(());
        }
        self.apply_now(vec![op])
    }

    fn apply_now(&self, ops: Vec<Op>) -> PyResult<()> {
        let (_, rev) = self.engine.snapshot(&self.id).map_err(err)?;
        let r = self
            .engine
            .apply_ops(&self.id, rev, ops, false)
            .map_err(err)?;
        if r.applied {
            Ok(())
        } else {
            let msgs: Vec<String> = r
                .diagnostics
                .iter()
                .map(|d| format!("{}: {}", d.code, d.message))
                .collect();
            Err(PyValueError::new_err(msgs.join("; ")))
        }
    }
}

#[pymethods]
impl Project {
    /// The project handle used by the engine.
    #[getter]
    fn id(&self) -> String {
        self.id.clone()
    }

    /// Current revision.
    #[getter]
    fn revision(&self) -> PyResult<u64> {
        Ok(self.engine.snapshot(&self.id).map_err(err)?.1.0)
    }

    /// Song title.
    #[getter]
    fn title(&self) -> PyResult<String> {
        Ok(self.doc()?.meta.title.clone())
    }

    /// A track by name (case-insensitive) or id.
    fn track(slf: Bound<'_, Self>, name_or_id: Bound<'_, PyAny>) -> PyResult<Track> {
        let r = match name_or_id.extract::<u64>() {
            Ok(id) => TrackRef::Id(id),
            Err(_) => TrackRef::Name(name_or_id.extract::<String>()?),
        };
        let doc = slf.borrow().doc()?;
        let id = resolve_track(&doc, &r).map_err(err)?;
        Ok(Track {
            project: slf.unbind(),
            id,
        })
    }

    /// All tracks.
    fn tracks(slf: Bound<'_, Self>) -> PyResult<Vec<Track>> {
        let doc = slf.borrow().doc()?;
        Ok(doc
            .tracks
            .iter()
            .map(|t| Track {
                project: slf.clone().unbind(),
                id: t.id,
            })
            .collect())
    }

    /// Section names.
    fn sections(&self) -> PyResult<Vec<String>> {
        Ok(self
            .doc()?
            .sections
            .iter()
            .map(|s| s.name.clone())
            .collect())
    }

    /// Groups the edits made inside a `with` block into one undo step.
    fn transaction(slf: Bound<'_, Self>, label: String) -> Transaction {
        Transaction {
            project: slf.unbind(),
            label,
        }
    }

    /// Applies raw ops given as JSON (an op object or an array of ops).
    fn apply_json(&self, ops: &str) -> PyResult<()> {
        let ops: Vec<Op> = match serde_json::from_str::<Vec<Op>>(ops) {
            Ok(v) => v,
            Err(_) => vec![
                serde_json::from_str::<Op>(ops)
                    .map_err(|e| PyValueError::new_err(e.to_string()))?,
            ],
        };
        for op in ops {
            self.submit(op)?;
        }
        Ok(())
    }

    /// Compact text overview of the project.
    fn describe(&self) -> PyResult<String> {
        self.engine.describe_project(&self.id).map_err(err)
    }

    /// Renders and analyzes. `quality` is "preview" or "final"; `start`
    /// and `end` (ticks) limit rendering to overlapping phrases.
    #[pyo3(signature = (quality = "preview", start = None, end = None))]
    fn render(
        &self,
        py: Python<'_>,
        quality: &str,
        start: Option<i64>,
        end: Option<i64>,
    ) -> PyResult<Report> {
        let q = match quality {
            "preview" => Quality::Preview,
            "final" => Quality::Final,
            other => {
                return Err(PyValueError::new_err(format!(
                    "quality must be preview or final, not {other:?}"
                )));
            }
        };
        let range = match (start, end) {
            (None, None) => None,
            (s, e) => Some(TickRange {
                start: s.unwrap_or(0),
                end: e.unwrap_or(i64::MAX / 4),
            }),
        };
        let engine = self.engine.clone();
        let id = self.id.clone();
        let summary = py
            .detach(move || engine.render(&id, range, q))
            .map_err(err)?;
        Ok(Report { summary })
    }

    /// Saves to `path`, or where the project was opened from.
    #[pyo3(signature = (path = None))]
    fn save(&self, path: Option<std::path::PathBuf>) -> PyResult<String> {
        let p = self
            .engine
            .save_project(&self.id, path.as_deref())
            .map_err(err)?;
        Ok(p.display().to_string())
    }

    fn __repr__(&self) -> PyResult<String> {
        Ok(format!(
            "<cantor.Project {:?} rev {}>",
            self.title()?,
            self.revision()?
        ))
    }
}

/// A `with` block that applies its edits atomically on exit.
#[pyclass(module = "cantor")]
struct Transaction {
    project: Py<Project>,
    label: String,
}

#[pymethods]
impl Transaction {
    fn __enter__(&self, py: Python<'_>) -> PyResult<()> {
        let p = self.project.borrow(py);
        let mut txn = p.txn.lock().expect("txn lock");
        if txn.is_some() {
            return Err(CantorError::new_err("transactions do not nest"));
        }
        *txn = Some((self.label.clone(), Vec::new()));
        Ok(())
    }

    fn __exit__(
        &self,
        py: Python<'_>,
        exc_type: Option<Bound<'_, PyType>>,
        _exc: Option<Bound<'_, PyAny>>,
        _tb: Option<Bound<'_, PyAny>>,
    ) -> PyResult<bool> {
        let p = self.project.borrow(py);
        let taken = p.txn.lock().expect("txn lock").take();
        if exc_type.is_some() {
            return Ok(false);
        }
        if let Some((label, ops)) = taken
            && !ops.is_empty()
        {
            p.apply_now(vec![Op::Batch { label, ops }])?;
        }
        Ok(false)
    }
}

/// A track.
#[pyclass(module = "cantor")]
struct Track {
    project: Py<Project>,
    id: TrackId,
}

#[pymethods]
impl Track {
    /// Track id.
    #[getter]
    fn id(&self) -> u64 {
        self.id.0
    }

    /// Track name.
    #[getter]
    fn name(&self, py: Python<'_>) -> PyResult<String> {
        let doc = self.project.borrow(py).doc()?;
        Ok(doc
            .track(self.id)
            .map(|t| t.name.clone())
            .unwrap_or_default())
    }

    /// Notes in time order, optionally only those starting in a section.
    #[pyo3(signature = (section = None))]
    fn notes(&self, py: Python<'_>, section: Option<&str>) -> PyResult<Vec<Note>> {
        let doc = self.project.borrow(py).doc()?;
        let track = doc
            .track(self.id)
            .ok_or_else(|| CantorError::new_err(format!("{} no longer exists", self.id)))?;
        let bounds = match section {
            Some(name) => {
                let s = doc
                    .section(name)
                    .ok_or_else(|| CantorError::new_err(format!("no section named {name:?}")))?;
                Some((s.start, s.end))
            }
            None => None,
        };
        let mut notes: Vec<(i64, NoteId)> = track
            .parts
            .iter()
            .flat_map(|p| p.notes.iter().map(move |n| (p.start + n.start, n.id)))
            .filter(|(start, _)| bounds.is_none_or(|(a, b)| *start >= a && *start < b))
            .collect();
        notes.sort();
        Ok(notes
            .into_iter()
            .map(|(_, id)| Note {
                project: self.project.clone_ref(py),
                track: self.id,
                id,
            })
            .collect())
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        Ok(format!("<cantor.Track {} {:?}>", self.id.0, self.name(py)?))
    }
}

/// A note.
#[pyclass(module = "cantor")]
struct Note {
    project: Py<Project>,
    track: TrackId,
    id: NoteId,
}

impl Note {
    fn with<T>(
        &self,
        py: Python<'_>,
        f: impl FnOnce(&Doc, i64, &cantor_core::Note) -> T,
    ) -> PyResult<T> {
        let doc = self.project.borrow(py).doc()?;
        let (_, part, n) = doc
            .note(self.id)
            .ok_or_else(|| CantorError::new_err(format!("{} no longer exists", self.id)))?;
        Ok(f(&doc, part.start + n.start, n))
    }
}

#[pymethods]
impl Note {
    /// Note id.
    #[getter]
    fn id(&self) -> u64 {
        self.id.0
    }

    /// Lyric.
    #[getter]
    fn lyric(&self, py: Python<'_>) -> PyResult<String> {
        self.with(py, |_, _, n| n.lyric.clone())
    }

    #[setter]
    fn set_lyric(&self, py: Python<'_>, lyric: String) -> PyResult<()> {
        self.project
            .borrow(py)
            .submit(Op::SetLyric { id: self.id, lyric })
    }

    /// Pitch in cents (MIDI × 100).
    #[getter]
    fn pitch_cents(&self, py: Python<'_>) -> PyResult<f64> {
        self.with(py, |_, _, n| n.pitch_cents)
    }

    /// Absolute start tick.
    #[getter]
    fn start(&self, py: Python<'_>) -> PyResult<i64> {
        self.with(py, |_, start, _| start)
    }

    /// Length in ticks.
    #[getter]
    fn length(&self, py: Python<'_>) -> PyResult<i64> {
        self.with(py, |_, _, n| n.length)
    }

    /// Whether this is the last note of its phrase.
    #[getter]
    fn is_phrase_end(&self, py: Python<'_>) -> PyResult<bool> {
        let doc = self.project.borrow(py).doc()?;
        let track = doc
            .track(self.track)
            .ok_or_else(|| CantorError::new_err("track no longer exists"))?;
        Ok(phrase_spans(&doc, track)
            .iter()
            .any(|s| s.notes.last() == Some(&self.id)))
    }

    /// A curve parameter of this note's track, limited to this note.
    fn curve(&self, py: Python<'_>, param: &str) -> PyResult<Curve> {
        if cantor_core::params::lookup(param).is_none() {
            return Err(PyValueError::new_err(format!(
                "unknown parameter {param:?}"
            )));
        }
        Ok(Curve {
            note: Note {
                project: self.project.clone_ref(py),
                track: self.track,
                id: self.id,
            },
            param: param.to_owned(),
        })
    }

    /// Adds vibrato. `start` is the fraction of the note where it begins.
    #[pyo3(signature = (depth_cents, rate_hz, start = 0.25, fade_in = 0.2, fade_out = 0.2, drift = 0.5))]
    #[allow(clippy::too_many_arguments)] // Python keyword arguments
    fn vibrato(
        &self,
        py: Python<'_>,
        depth_cents: f64,
        rate_hz: f64,
        start: f64,
        fade_in: f64,
        fade_out: f64,
        drift: f64,
    ) -> PyResult<()> {
        self.project.borrow(py).submit(Op::SetNoteExpr {
            id: self.id,
            expr: NoteExprPatch {
                vibrato: Some(Some(Vibrato {
                    depth_cents,
                    rate_hz,
                    start,
                    fade_in,
                    fade_out,
                    phase: 0.0,
                    drift,
                })),
                ..NoteExprPatch::default()
            },
        })
    }

    /// Moves the note by `dt` ticks and `dcents` cents (curves move along).
    #[pyo3(signature = (dt = 0, dcents = 0.0))]
    fn move_by(&self, py: Python<'_>, dt: i64, dcents: f64) -> PyResult<()> {
        self.project.borrow(py).submit(Op::MoveNotes {
            ids: vec![self.id],
            dt,
            dcents,
            carry_curves: true,
        })
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let (lyric, cents) = self.with(py, |_, _, n| (n.lyric.clone(), n.pitch_cents))?;
        Ok(format!(
            "<cantor.Note {} {lyric:?} {cents} cents>",
            self.id.0
        ))
    }
}

/// A note's slice of a track curve (the user edit layer).
#[pyclass(module = "cantor")]
struct Curve {
    note: Note,
    param: String,
}

/// Parses "last 30%", "first 20%", "all" into a fraction range of a note.
fn parse_over(over: &str) -> PyResult<(f64, f64)> {
    let o = over.trim().to_lowercase();
    if o == "all" || o.is_empty() {
        return Ok((0.0, 1.0));
    }
    let (which, pct) = o.split_once(' ').ok_or_else(|| {
        PyValueError::new_err(format!("cannot parse over={over:?}; use e.g. \"last 30%\""))
    })?;
    let frac = pct
        .trim_end_matches('%')
        .parse::<f64>()
        .map_err(|_| PyValueError::new_err(format!("cannot parse percentage in {over:?}")))?
        / 100.0;
    let frac = frac.clamp(0.0, 1.0);
    match which {
        "last" => Ok((1.0 - frac, 1.0)),
        "first" => Ok((0.0, frac)),
        _ => Err(PyValueError::new_err(format!(
            "over must start with first/last, got {over:?}"
        ))),
    }
}

#[pymethods]
impl Curve {
    /// Ramps the edit layer linearly from `from_` to `to` over part of the
    /// note ("last 30%", "first 20%", "all"). Values are in the parameter's
    /// unit (see the parameter registry).
    #[pyo3(signature = (to, over = "all", from_ = 0.0))]
    fn ramp(&self, py: Python<'_>, to: f64, over: &str, from_: f64) -> PyResult<()> {
        let (a, b) = parse_over(over)?;
        let (start, len) = self.note.with(py, |_, s, n| (s, n.length))?;
        let s = start + (a * len as f64).round() as i64;
        let e = (start + (b * len as f64).round() as i64).max(s + 2);
        self.note.project.borrow(py).submit(Op::SetCurve {
            track: self.note.track,
            param: ParamId::new(self.param.clone()),
            range: TickRange { start: s, end: e },
            points: vec![(s, from_), (e - 1, to)],
            interp: Interp::Linear,
        })
    }

    /// Sets the edit layer to a constant over the whole note.
    fn set(&self, py: Python<'_>, value: f64) -> PyResult<()> {
        self.ramp(py, value, "all", value)
    }
}

/// A render with its analysis.
#[pyclass(module = "cantor")]
struct Report {
    summary: RenderSummary,
}

/// Per-note measurements. Unmeasured values are NaN.
#[pyclass(module = "cantor", get_all, skip_from_py_object)]
#[derive(Clone)]
struct NoteMetric {
    id: u64,
    lyric: String,
    pitch_error_mean: f64,
    pitch_error_p95: f64,
    pitch_error_max: f64,
    pitch_bias: f64,
    onset_error_ms: f64,
    voiced_ratio: f64,
}

#[pymethods]
impl NoteMetric {
    fn __repr__(&self) -> String {
        format!(
            "<cantor.NoteMetric {} {:?} p95 {:.1} cents>",
            self.id, self.lyric, self.pitch_error_p95
        )
    }
}

#[pymethods]
impl Report {
    /// Per-note measurements.
    #[getter]
    fn notes(&self) -> Vec<NoteMetric> {
        let Some(a) = &self.summary.analysis else {
            return Vec::new();
        };
        let nan = |x: Option<f64>| x.unwrap_or(f64::NAN);
        a.notes()
            .map(|n| NoteMetric {
                id: n.id.0,
                lyric: n.lyric.clone(),
                pitch_error_mean: nan(n.pitch_error_mean),
                pitch_error_p95: nan(n.pitch_error_p95),
                pitch_error_max: nan(n.pitch_error_max),
                pitch_bias: nan(n.pitch_bias),
                onset_error_ms: nan(n.onset_error_ms),
                voiced_ratio: n.voiced_ratio,
            })
            .collect()
    }

    /// Path of the rendered WAV.
    #[getter]
    fn audio_path(&self) -> String {
        self.summary
            .audio_uri
            .trim_start_matches("file://")
            .to_owned()
    }

    /// BLAKE3 hash of the audio.
    #[getter]
    fn audio_hash(&self) -> String {
        self.summary.audio_hash.clone()
    }

    /// The render handle (for comparisons through the server API).
    #[getter]
    fn render_id(&self) -> String {
        self.summary.render_id.clone()
    }

    /// The full report as JSON.
    fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.summary).expect("reports serialize")
    }

    /// A short human-readable summary.
    fn summary(&self) -> String {
        let mut s = format!(
            "{:.2} s rendered, {} phrases",
            self.summary.seconds,
            self.summary.trace.phrases.len()
        );
        if let Some(a) = &self.summary.analysis {
            for t in &a.tracks {
                s += &format!(
                    "; track {}: pitch p95 {} cents, timing mean {} ms",
                    t.track.0,
                    t.pitch
                        .p95_cents
                        .map_or("n/a".into(), |x| format!("{x:.1}")),
                    t.timing
                        .mean_abs_ms
                        .map_or("n/a".into(), |x| format!("{x:.0}")),
                );
            }
            if let Some(l) = a.loudness.integrated_lufs {
                s += &format!("; {l:.1} LUFS");
            }
            s += &format!("; {} artifacts", a.artifacts.len());
        }
        if !self.summary.diagnostics.is_empty() {
            s += &format!("; {} diagnostics", self.summary.diagnostics.len());
        }
        s
    }

    fn __repr__(&self) -> String {
        format!("<cantor.Report {}>", self.summary())
    }
}

fn wrap(engine: Arc<Engine>, id: String) -> Project {
    Project {
        engine,
        id,
        txn: Mutex::new(None),
    }
}

/// Opens a `.cantor` project.
#[pyfunction]
fn open(path: std::path::PathBuf) -> PyResult<Project> {
    let e = engine();
    let info = e.open_project(&path).map_err(err)?;
    Ok(wrap(e, info.project_id))
}

/// Creates an empty project.
#[pyfunction]
#[pyo3(signature = (title = ""))]
fn new(title: &str) -> Project {
    let e = engine();
    let info = e.new_project(title);
    wrap(e, info.project_id)
}

/// Imports USTX, MIDI or MusicXML as a new project.
#[pyfunction]
fn import_file(path: std::path::PathBuf) -> PyResult<Project> {
    let e = engine();
    let info = e.import_file(&path).map_err(err)?;
    Ok(wrap(e, info.project_id))
}

/// The native module behind the `cantor` package.
#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("CantorError", m.py().get_type::<CantorError>())?;
    m.add_class::<Project>()?;
    m.add_class::<Transaction>()?;
    m.add_class::<Track>()?;
    m.add_class::<Note>()?;
    m.add_class::<Curve>()?;
    m.add_class::<Report>()?;
    m.add_class::<NoteMetric>()?;
    m.add_function(wrap_pyfunction!(open, m)?)?;
    m.add_function(wrap_pyfunction!(new, m)?)?;
    m.add_function(wrap_pyfunction!(import_file, m)?)?;
    Ok(())
}
