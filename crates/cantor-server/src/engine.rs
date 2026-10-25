//! The engine: open projects, edits with optimistic concurrency, history,
//! renders with analysis, and the singer registry. JSON-RPC, MCP and
//! Python are thin layers over these methods.

use crate::describe;
use crate::singers::{self, SingerInfo, SingerSchema};
use cantor_analysis::AnalysisReport;
use cantor_backends::DiffSinger;
use cantor_backends::rmvpe::Rmvpe;
use cantor_core::{
    ApplyResult, Diagnostic, DocDiff, Document, HistoryNodeId, Op, PhraseId, Project, RetakeDim,
    Revision, TakeId, TickRange, TrackId,
};
use cantor_io::project_file::{self, Compression};
use cantor_phon::wasm::PluginSet;
use cantor_render::cache::DiskCache;
use cantor_render::render::{Quality, RenderTrace, Renderer};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Where the engine finds singers and writes caches and renders.
#[derive(Clone, Debug)]
pub struct EngineConfig {
    /// Directories containing voicebank directories.
    pub singer_paths: Vec<PathBuf>,
    /// Directories containing vocoder directories.
    pub vocoder_paths: Vec<PathBuf>,
    /// Directories containing phonemizer plugins (`*.wasm`).
    pub phonemizer_paths: Vec<PathBuf>,
    /// RMVPE model for analysis; `None` disables pitch analysis.
    pub rmvpe: Option<PathBuf>,
    /// Render cache.
    pub cache_dir: PathBuf,
    /// Where rendered WAV files are written.
    pub render_dir: PathBuf,
}

impl EngineConfig {
    /// From `CANTOR_SINGERS`, `CANTOR_VOCODERS`, `CANTOR_PHONEMIZER_PATH`
    /// (path lists), `CANTOR_RMVPE`, `CANTOR_CACHE` and `CANTOR_RENDERS`.
    pub fn from_env() -> Self {
        let paths = |var: &str| {
            std::env::var_os(var)
                .map(|v| std::env::split_paths(&v).collect())
                .unwrap_or_default()
        };
        let base = std::env::var_os("CANTOR_CACHE")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("cantor"));
        Self {
            singer_paths: paths("CANTOR_SINGERS"),
            vocoder_paths: paths("CANTOR_VOCODERS"),
            phonemizer_paths: paths("CANTOR_PHONEMIZER_PATH"),
            rmvpe: std::env::var_os("CANTOR_RMVPE").map(PathBuf::from),
            render_dir: std::env::var_os("CANTOR_RENDERS")
                .map(PathBuf::from)
                .unwrap_or_else(|| base.join("renders")),
            cache_dir: base.join("render"),
        }
    }
}

/// A loaded phonemizer plugin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PhonemizerInfo {
    /// Display name.
    pub name: String,
    /// Plugin version.
    pub version: String,
    /// Languages it phonemizes; it replaces voicebank dictionaries for them.
    pub languages: Vec<String>,
    /// The plugin file.
    pub path: String,
}

/// Phonemizer plugins and the plugins that failed to load.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PhonemizerList {
    /// Loaded plugins, in priority order.
    pub plugins: Vec<PhonemizerInfo>,
    /// Load errors, one per rejected plugin file.
    pub errors: Vec<String>,
}

/// An engine failure. Each maps to a stable code for JSON-RPC and MCP.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// A project, render, singer, checkpoint or track does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// `base_revision` is not the current revision.
    #[error("revision conflict: the project is at revision {current}, not {base}")]
    Conflict {
        /// The revision the caller based its edit on.
        base: Revision,
        /// The current revision.
        current: Revision,
        /// What changed since `base`.
        diff: DocDiff,
    },
    /// The request is malformed or unsupported.
    #[error("invalid request: {0}")]
    Invalid(String),
    /// Reading or writing a file failed.
    #[error("{0}")]
    Io(String),
    /// Rendering failed.
    #[error("render failed: {0}")]
    Render(String),
}

impl EngineError {
    /// Stable machine-readable code.
    pub fn code(&self) -> &'static str {
        match self {
            EngineError::NotFound(_) => "not_found",
            EngineError::Conflict { .. } => "conflict",
            EngineError::Invalid(_) => "invalid",
            EngineError::Io(_) => "io",
            EngineError::Render(_) => "render",
        }
    }
}

type Result<T> = std::result::Result<T, EngineError>;

/// A track named by id or by (case-insensitive) name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum TrackRef {
    /// Track id.
    Id(u64),
    /// Track name.
    Name(String),
}

/// A newly opened project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectInfo {
    /// Handle for later calls.
    pub project_id: String,
    /// Current revision; pass it as `base_revision` when editing.
    pub revision: Revision,
    /// Title.
    pub title: String,
    /// Diagnostics from loading, migrating or importing.
    pub diagnostics: Vec<Diagnostic>,
}

/// A summary of a render.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct RenderSummary {
    /// Handle for `compare`.
    pub render_id: String,
    /// The project revision rendered.
    pub revision: Revision,
    /// `file://` URI of the WAV.
    pub audio_uri: String,
    /// BLAKE3 of the samples.
    pub audio_hash: String,
    /// Length in seconds.
    pub seconds: f64,
    /// Measurements, if an F0 extractor is configured.
    pub analysis: Option<AnalysisReport>,
    /// What ran per phrase.
    pub trace: RenderTrace,
    /// Everything that could not be honored.
    pub diagnostics: Vec<Diagnostic>,
}

/// Metric changes from one render to another (`b − a`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Comparison {
    /// The baseline render.
    pub render_a: String,
    /// The compared render.
    pub render_b: String,
    /// Whether the audio is identical.
    pub identical_audio: bool,
    /// Named metric deltas.
    pub metrics: BTreeMap<String, MetricDelta>,
    /// Per-note p95 pitch-error deltas, by note id.
    pub notes: Vec<NoteDelta>,
}

/// One metric before and after.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MetricDelta {
    /// Value in render A.
    pub a: Option<f64>,
    /// Value in render B.
    pub b: Option<f64>,
    /// `b − a`.
    pub delta: Option<f64>,
}

/// A note's p95 pitch error before and after.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NoteDelta {
    /// The note.
    pub id: cantor_core::NoteId,
    /// Lyric.
    pub lyric: String,
    /// p95 error in render A, cents.
    pub a: Option<f64>,
    /// p95 error in render B, cents.
    pub b: Option<f64>,
}

/// An entry of the undo tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HistoryEntry {
    /// Node id.
    pub node: HistoryNodeId,
    /// Parent node.
    pub parent: Option<HistoryNodeId>,
    /// Label.
    pub label: String,
    /// Revision when the edit was made.
    pub revision: Revision,
    /// Whether this is the current state.
    pub current: bool,
    /// Checkpoint names pointing here.
    pub checkpoints: Vec<String>,
}

struct OpenProject {
    doc: Document,
    path: Option<PathBuf>,
    checkpoints: BTreeMap<String, HistoryNodeId>,
    /// Diffs per revision, for conflict reports.
    log: Vec<(Revision, DocDiff)>,
}

impl OpenProject {
    fn new(project: Project, path: Option<PathBuf>) -> Self {
        Self {
            doc: Document::new(project),
            path,
            checkpoints: BTreeMap::new(),
            log: Vec::new(),
        }
    }

    fn check_base(&self, base: Revision) -> Result<()> {
        let current = self.doc.revision();
        if base == current {
            return Ok(());
        }
        let mut diff = DocDiff::default();
        for (rev, d) in &self.log {
            if *rev > base {
                diff.merge(d.clone());
            }
        }
        Err(EngineError::Conflict {
            base,
            current,
            diff,
        })
    }

    fn record(&mut self, r: &ApplyResult) {
        if r.applied {
            self.log.push((r.revision, r.diff.clone()));
        }
    }
}

struct RenderRecord {
    summary: RenderSummary,
}

/// The Cantor engine.
pub struct Engine {
    config: EngineConfig,
    projects: Mutex<BTreeMap<String, Arc<Mutex<OpenProject>>>>,
    renders: Mutex<BTreeMap<String, RenderRecord>>,
    backends: Mutex<HashMap<String, Arc<DiffSinger>>>,
    rmvpe: OnceLock<Option<Arc<Rmvpe>>>,
    phonemizers: OnceLock<(Arc<PluginSet>, Vec<String>)>,
    next: AtomicU64,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("config", &self.config)
            .finish()
    }
}

impl Engine {
    /// Creates an engine.
    pub fn new(config: EngineConfig) -> Self {
        Self {
            config,
            projects: Mutex::new(BTreeMap::new()),
            renders: Mutex::new(BTreeMap::new()),
            backends: Mutex::new(HashMap::new()),
            rmvpe: OnceLock::new(),
            phonemizers: OnceLock::new(),
            next: AtomicU64::new(1),
        }
    }

    /// The configuration.
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    fn fresh(&self, prefix: &str) -> String {
        format!("{prefix}-{}", self.next.fetch_add(1, Ordering::Relaxed))
    }

    fn project(&self, id: &str) -> Result<Arc<Mutex<OpenProject>>> {
        self.projects
            .lock()
            .expect("projects lock")
            .get(id)
            .cloned()
            .ok_or_else(|| EngineError::NotFound(format!("project `{id}`")))
    }

    fn insert(
        &self,
        project: Project,
        path: Option<PathBuf>,
        diagnostics: Vec<Diagnostic>,
    ) -> ProjectInfo {
        let id = self.fresh("p");
        let info = ProjectInfo {
            project_id: id.clone(),
            revision: Revision(0),
            title: project.meta.title.clone(),
            diagnostics,
        };
        self.projects
            .lock()
            .expect("projects lock")
            .insert(id, Arc::new(Mutex::new(OpenProject::new(project, path))));
        info
    }

    /// Opens a `.cantor` file.
    pub fn open_project(&self, path: &Path) -> Result<ProjectInfo> {
        let loaded = project_file::load(path).map_err(|e| EngineError::Io(e.to_string()))?;
        Ok(self.insert(loaded.project, Some(path.to_path_buf()), loaded.diagnostics))
    }

    /// Creates an empty project.
    pub fn new_project(&self, title: &str) -> ProjectInfo {
        let mut p = Project::new();
        p.meta.title = title.to_owned();
        self.insert(p, None, Vec::new())
    }

    /// Opens an in-memory project.
    pub fn add_project(&self, project: Project) -> ProjectInfo {
        self.insert(project, None, Vec::new())
    }

    /// Imports USTX, MIDI or MusicXML as a new project.
    pub fn import_file(&self, path: &Path) -> Result<ProjectInfo> {
        let imported = cantor_io::import::import_path(path)
            .map_err(|e| EngineError::Invalid(e.to_string()))?;
        Ok(self.insert(imported.project, None, imported.diagnostics))
    }

    /// Closes a project without saving.
    pub fn close_project(&self, id: &str) -> Result<()> {
        self.projects
            .lock()
            .expect("projects lock")
            .remove(id)
            .map(|_| ())
            .ok_or_else(|| EngineError::NotFound(format!("project `{id}`")))
    }

    /// Saves a project to `path`, or where it was opened from.
    pub fn save_project(&self, id: &str, path: Option<&Path>) -> Result<PathBuf> {
        let p = self.project(id)?;
        let mut p = p.lock().expect("project lock");
        let target = path
            .map(Path::to_path_buf)
            .or_else(|| p.path.clone())
            .ok_or_else(|| EngineError::Invalid("this project has no path yet; pass one".into()))?;
        project_file::save(&target, p.doc.project(), Compression::None)
            .map_err(|e| EngineError::Io(format!("{}: {e}", target.display())))?;
        p.path = Some(target.clone());
        Ok(target)
    }

    /// A snapshot of the project and its revision.
    pub fn snapshot(&self, id: &str) -> Result<(Arc<Project>, Revision)> {
        let p = self.project(id)?;
        let p = p.lock().expect("project lock");
        Ok((p.doc.snapshot(), p.doc.revision()))
    }

    /// Compact text overview: tracks, sections, phrases and notes with
    /// lyrics, phonemes and timing.
    pub fn describe_project(&self, id: &str) -> Result<String> {
        let (project, rev) = self.snapshot(id)?;
        Ok(describe::project(&project, rev, &|t| self.phonemes_fn(t)))
    }

    /// A slice of the project between two ticks, with curve summaries.
    pub fn describe_range(
        &self,
        id: &str,
        track: Option<&TrackRef>,
        range: TickRange,
        detail: describe::Detail,
    ) -> Result<String> {
        let (project, rev) = self.snapshot(id)?;
        let track = match track {
            Some(r) => Some(resolve_track(&project, r)?),
            None => None,
        };
        Ok(describe::range(&project, rev, track, range, detail, &|t| {
            self.phonemes_fn(t)
        }))
    }

    /// Dictionary phonemes for a lyric as `singer` would sing it, if the
    /// singer is installed.
    pub fn phonemes(&self, singer: &str, lyric: &str, lang: Option<&str>) -> Option<Vec<String>> {
        self.backend_for(singer).ok()?.phonemes_for(lyric, lang)
    }

    fn phonemes_fn(&self, track: &cantor_core::Track) -> Option<Arc<DiffSinger>> {
        self.backend_for(&track.singer.id).ok()
    }

    /// Applies ops atomically (as one batch) if `base_revision` is current.
    pub fn apply_ops(
        &self,
        id: &str,
        base_revision: Revision,
        ops: Vec<Op>,
        dry_run: bool,
    ) -> Result<ApplyResult> {
        let p = self.project(id)?;
        let mut p = p.lock().expect("project lock");
        p.check_base(base_revision)?;
        let op = match ops.len() {
            1 => ops.into_iter().next().expect("one op"),
            n => Op::Batch {
                label: format!("{n} edits"),
                ops,
            },
        };
        if dry_run {
            return Ok(p.doc.dry_run(&op));
        }
        let r = p.doc.apply(op);
        p.record(&r);
        Ok(r)
    }

    /// Generates takes for the phrases of `track` overlapping `range`.
    #[allow(clippy::too_many_arguments)]
    pub fn retake(
        &self,
        id: &str,
        base_revision: Revision,
        track: &TrackRef,
        range: TickRange,
        dims: Vec<RetakeDim>,
        n_variants: u8,
        seed: Option<u64>,
    ) -> Result<ApplyResult> {
        let (project, _) = self.snapshot(id)?;
        let track = resolve_track(&project, track)?;
        self.apply_ops(
            id,
            base_revision,
            vec![Op::Retake {
                track,
                range,
                dims,
                n_variants,
                seed,
            }],
            false,
        )
    }

    /// Selects a phrase's take (`None` = the original).
    pub fn select_take(
        &self,
        id: &str,
        base_revision: Revision,
        phrase: PhraseId,
        take: Option<TakeId>,
    ) -> Result<ApplyResult> {
        self.apply_ops(
            id,
            base_revision,
            vec![Op::SelectTake { phrase, take }],
            false,
        )
    }

    /// Undoes the last edit. Returns `None` at the start of history.
    pub fn undo(&self, id: &str) -> Result<Option<ApplyResult>> {
        let p = self.project(id)?;
        let mut p = p.lock().expect("project lock");
        let r = p.doc.undo();
        if let Some(r) = &r {
            p.record(r);
        }
        Ok(r)
    }

    /// Redoes along the most recent branch. Returns `None` if nothing to redo.
    pub fn redo(&self, id: &str) -> Result<Option<ApplyResult>> {
        let p = self.project(id)?;
        let mut p = p.lock().expect("project lock");
        let r = p.doc.redo();
        if let Some(r) = &r {
            p.record(r);
        }
        Ok(r)
    }

    /// Names the current state.
    pub fn checkpoint(&self, id: &str, name: &str) -> Result<HistoryEntry> {
        let p = self.project(id)?;
        let mut p = p.lock().expect("project lock");
        let node = p.doc.history().current();
        p.checkpoints.insert(name.to_owned(), node);
        Ok(history_entries(&p)
            .into_iter()
            .find(|e| e.node == node)
            .expect("current node"))
    }

    /// Returns to a checkpoint (or a history node given as `#<n>`).
    pub fn restore(&self, id: &str, base_revision: Revision, name: &str) -> Result<ApplyResult> {
        let p = self.project(id)?;
        let mut p = p.lock().expect("project lock");
        p.check_base(base_revision)?;
        let node = match p.checkpoints.get(name) {
            Some(n) => *n,
            None => name
                .strip_prefix('#')
                .and_then(|n| n.parse().ok())
                .map(HistoryNodeId)
                .ok_or_else(|| EngineError::NotFound(format!("checkpoint `{name}`")))?,
        };
        let r = p
            .doc
            .checkout(node)
            .ok_or_else(|| EngineError::NotFound(format!("history node {}", node.0)))?;
        p.record(&r);
        Ok(r)
    }

    /// The undo tree with checkpoints.
    pub fn list_history(&self, id: &str) -> Result<Vec<HistoryEntry>> {
        let p = self.project(id)?;
        let p = p.lock().expect("project lock");
        Ok(history_entries(&p))
    }

    /// Singers on the search path.
    pub fn list_singers(&self) -> Vec<SingerInfo> {
        singers::scan(&self.config.singer_paths)
            .into_iter()
            .map(|(i, _)| i)
            .collect()
    }

    /// A singer's parameters, languages, modes and licence.
    pub fn get_singer_schema(&self, singer: &str) -> Result<SingerSchema> {
        let (info, _) = self.find_singer(singer)?;
        let backend = self.backend_for(singer)?;
        let xsampa = backend
            .languages()
            .into_iter()
            .filter_map(|lang| {
                let map = backend.xsampa(&lang)?;
                let pairs = map
                    .pairs()
                    .map(|(n, x)| (n.to_owned(), x.to_owned()))
                    .collect();
                Some((lang, pairs))
            })
            .collect();
        Ok(singers::schema(
            info,
            cantor_render::backend::Backend::params(backend.as_ref()),
            xsampa,
        ))
    }

    fn find_singer(&self, singer: &str) -> Result<(SingerInfo, PathBuf)> {
        singers::scan(&self.config.singer_paths)
            .into_iter()
            .find(|(i, _)| i.id == singer)
            .ok_or_else(|| {
                EngineError::NotFound(format!(
                    "singer `{singer}` (searched {})",
                    self.config
                        .singer_paths
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })
    }

    fn backend_for(&self, singer: &str) -> Result<Arc<DiffSinger>> {
        if let Some(b) = self.backends.lock().expect("backends lock").get(singer) {
            return Ok(b.clone());
        }
        let (info, dir) = self.find_singer(singer)?;
        let vocoder = find_vocoder(&self.config.vocoder_paths, &info.vocoder)
            .ok_or_else(|| EngineError::NotFound(format!("vocoder `{}`", info.vocoder)))?;
        let b = Arc::new(
            DiffSinger::load(&dir, &vocoder)
                .map_err(|e| EngineError::Render(e.to_string()))?
                .with_phonemizers(self.phonemizers().0.clone()),
        );
        self.backends
            .lock()
            .expect("backends lock")
            .insert(singer.to_owned(), b.clone());
        Ok(b)
    }

    /// Phonemizer plugins on the configured path, loaded once, with the
    /// errors of plugins that failed to load.
    fn phonemizers(&self) -> &(Arc<PluginSet>, Vec<String>) {
        self.phonemizers.get_or_init(|| {
            let (set, errors) = PluginSet::scan(&self.config.phonemizer_paths);
            for e in &errors {
                tracing::warn!("{e}");
            }
            (
                Arc::new(set),
                errors.iter().map(|e| e.to_string()).collect(),
            )
        })
    }

    /// Loaded phonemizer plugins and plugin load errors.
    pub fn list_phonemizers(&self) -> PhonemizerList {
        let (set, errors) = self.phonemizers();
        PhonemizerList {
            plugins: set
                .plugins()
                .iter()
                .map(|p| PhonemizerInfo {
                    name: p.name().to_owned(),
                    version: p.version().to_owned(),
                    languages: p.languages().to_vec(),
                    path: p.path().display().to_string(),
                })
                .collect(),
            errors: errors.clone(),
        }
    }

    fn analyzer(&self) -> Option<Arc<Rmvpe>> {
        self.rmvpe
            .get_or_init(|| {
                self.config
                    .rmvpe
                    .as_ref()
                    .and_then(|p| Rmvpe::load(p).ok().map(Arc::new))
            })
            .clone()
    }

    /// Renders a project (or the phrases overlapping `range`) to a WAV and
    /// returns the analysis report.
    pub fn render(
        &self,
        id: &str,
        range: Option<TickRange>,
        quality: Quality,
    ) -> Result<RenderSummary> {
        let (project, revision) = self.snapshot(id)?;
        let mut singers: Vec<&str> = project
            .tracks
            .iter()
            .map(|t| t.singer.id.as_str())
            .collect();
        singers.sort_unstable();
        singers.dedup();
        let singer = match singers.as_slice() {
            [one] => *one,
            [] => return Err(EngineError::Invalid("the project has no tracks".into())),
            _ => {
                return Err(EngineError::Invalid(format!(
                    "tracks use several singers ({}); render one singer per call for now",
                    singers.join(", ")
                )));
            }
        };
        let backend = self.backend_for(singer)?;
        let cache = DiskCache::new(&self.config.cache_dir)
            .map_err(|e| EngineError::Io(format!("{}: {e}", self.config.cache_dir.display())))?;
        let mut renderer = Renderer::new(backend, cache);
        let mut diagnostics = Vec::new();
        match self.analyzer() {
            Some(a) => renderer = renderer.with_analyzer(a),
            None => diagnostics.push(Diagnostic::warning(
                "render.analysis_unavailable",
                "no RMVPE model configured (CANTOR_RMVPE); pitch and timing analysis are skipped",
                None,
            )),
        }
        let out = renderer
            .render_range(&project, quality, range)
            .map_err(|e| EngineError::Render(e.to_string()))?;
        std::fs::create_dir_all(&self.config.render_dir)
            .map_err(|e| EngineError::Io(e.to_string()))?;
        let render_id = self.fresh("r");
        let wav = self.config.render_dir.join(format!("{render_id}.wav"));
        cantor_render::wav::write_stereo_f32(&wav, out.sample_rate, &out.samples)
            .map_err(|e| EngineError::Io(format!("{}: {e}", wav.display())))?;
        diagnostics.extend(out.diagnostics);
        let summary = RenderSummary {
            render_id: render_id.clone(),
            revision,
            audio_uri: format!("file://{}", wav.display()),
            audio_hash: out.audio_hash.hex(),
            seconds: out.samples.len() as f64 / 2.0 / f64::from(out.sample_rate),
            analysis: out.analysis,
            trace: out.trace,
            diagnostics,
        };
        self.renders.lock().expect("renders lock").insert(
            render_id,
            RenderRecord {
                summary: summary.clone(),
            },
        );
        Ok(summary)
    }

    /// A previous render's summary.
    pub fn get_render(&self, render_id: &str) -> Result<RenderSummary> {
        self.renders
            .lock()
            .expect("renders lock")
            .get(render_id)
            .map(|r| r.summary.clone())
            .ok_or_else(|| EngineError::NotFound(format!("render `{render_id}`")))
    }

    /// Metric deltas between two renders.
    pub fn compare(&self, render_a: &str, render_b: &str) -> Result<Comparison> {
        let a = self.get_render(render_a)?;
        let b = self.get_render(render_b)?;
        Ok(compare(&a, &b))
    }

    /// Exports a project: `cantor` (project file) or `wav` (final-quality
    /// render).
    pub fn export(&self, id: &str, format: &str, path: &Path) -> Result<PathBuf> {
        match format {
            "cantor" => {
                let (project, _) = self.snapshot(id)?;
                project_file::save(path, &project, Compression::None)
                    .map_err(|e| EngineError::Io(format!("{}: {e}", path.display())))?;
                Ok(path.to_path_buf())
            }
            "wav" => {
                let r = self.render(id, None, Quality::Final)?;
                let src = r.audio_uri.trim_start_matches("file://");
                std::fs::copy(src, path).map_err(|e| EngineError::Io(e.to_string()))?;
                Ok(path.to_path_buf())
            }
            other => Err(EngineError::Invalid(format!(
                "export format `{other}` is not supported (cantor, wav)"
            ))),
        }
    }
}

/// A vocoder directory named `name`, or whose `vocoder.yaml` declares it.
fn find_vocoder(paths: &[PathBuf], name: &str) -> Option<PathBuf> {
    let declares = |dir: &Path| {
        std::fs::read_to_string(dir.join("vocoder.yaml")).is_ok_and(|y| {
            y.lines().any(|l| {
                l.strip_prefix("name:")
                    .is_some_and(|v| v.trim().trim_matches('"') == name)
            })
        })
    };
    for base in paths {
        let direct = base.join(name);
        if direct.join("vocoder.yaml").is_file() {
            return Some(direct);
        }
        if declares(base) {
            return Some(base.clone());
        }
        let mut subdirs: Vec<PathBuf> = std::fs::read_dir(base)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir())
            .collect();
        subdirs.sort();
        if let Some(d) = subdirs.into_iter().find(|d| declares(d)) {
            return Some(d);
        }
    }
    None
}

/// Resolves a track reference.
pub fn resolve_track(project: &Project, r: &TrackRef) -> Result<TrackId> {
    match r {
        TrackRef::Id(id) => project
            .track(TrackId(*id))
            .map(|t| t.id)
            .ok_or_else(|| EngineError::NotFound(format!("track {id}"))),
        TrackRef::Name(name) => project
            .tracks
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
            .map(|t| t.id)
            .ok_or_else(|| EngineError::NotFound(format!("track `{name}`"))),
    }
}

fn history_entries(p: &OpenProject) -> Vec<HistoryEntry> {
    let current = p.doc.history().current();
    p.doc
        .history()
        .nodes()
        .iter()
        .map(|n| HistoryEntry {
            node: n.id,
            parent: n.parent,
            label: n.label.clone(),
            revision: n.revision,
            current: n.id == current,
            checkpoints: p
                .checkpoints
                .iter()
                .filter(|(_, v)| **v == n.id)
                .map(|(k, _)| k.clone())
                .collect(),
        })
        .collect()
}

fn compare(a: &RenderSummary, b: &RenderSummary) -> Comparison {
    let mut metrics = BTreeMap::new();
    let mut put = |name: &str, x: Option<f64>, y: Option<f64>| {
        metrics.insert(
            name.to_owned(),
            MetricDelta {
                a: x,
                b: y,
                delta: x.zip(y).map(|(x, y)| y - x),
            },
        );
    };
    let (aa, ba) = (a.analysis.as_ref(), b.analysis.as_ref());
    let first = |r: Option<&AnalysisReport>| r.and_then(|r| r.tracks.first()).cloned();
    let (ta, tb) = (first(aa), first(ba));
    put(
        "pitch_mean_cents",
        ta.as_ref().and_then(|t| t.pitch.mean_cents),
        tb.as_ref().and_then(|t| t.pitch.mean_cents),
    );
    put(
        "pitch_p95_cents",
        ta.as_ref().and_then(|t| t.pitch.p95_cents),
        tb.as_ref().and_then(|t| t.pitch.p95_cents),
    );
    put(
        "pitch_max_cents",
        ta.as_ref().and_then(|t| t.pitch.max_cents),
        tb.as_ref().and_then(|t| t.pitch.max_cents),
    );
    put(
        "octave_errors",
        ta.as_ref().map(|t| t.pitch.octave_errors as f64),
        tb.as_ref().map(|t| t.pitch.octave_errors as f64),
    );
    put(
        "timing_mean_abs_ms",
        ta.as_ref().and_then(|t| t.timing.mean_abs_ms),
        tb.as_ref().and_then(|t| t.timing.mean_abs_ms),
    );
    put(
        "integrated_lufs",
        aa.and_then(|r| r.loudness.integrated_lufs),
        ba.and_then(|r| r.loudness.integrated_lufs),
    );
    put(
        "true_peak_dbtp",
        aa.and_then(|r| r.loudness.true_peak_dbtp),
        ba.and_then(|r| r.loudness.true_peak_dbtp),
    );
    put(
        "artifacts",
        aa.map(|r| r.artifacts.len() as f64),
        ba.map(|r| r.artifacts.len() as f64),
    );
    let notes = match (aa, ba) {
        (Some(x), Some(y)) => x
            .notes()
            .map(|n| NoteDelta {
                id: n.id,
                lyric: n.lyric.clone(),
                a: n.pitch_error_p95,
                b: y.notes()
                    .find(|m| m.id == n.id)
                    .and_then(|m| m.pitch_error_p95),
            })
            .collect(),
        _ => Vec::new(),
    };
    Comparison {
        render_a: a.render_id.clone(),
        render_b: b.render_id.clone(),
        identical_audio: a.audio_hash == b.audio_hash,
        metrics,
        notes,
    }
}
