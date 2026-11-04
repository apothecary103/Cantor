//! The MCP server (docs/design.md "MCP tools"): the same engine methods the
//! HTTP API calls, so an agent and the person in the browser edit one
//! project and see each other's changes live. Resource `cantor://guide`
//! serves docs/agent-guide.md.

use crate::engine::{Caller, Engine, EngineError};
use cantor_core::{Op, Revision, TrackId, TuneRequest};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::*;
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

const GUIDE: &str = include_str!("../../../docs/agent-guide.md");
const GUIDE_URI: &str = "cantor://guide";

/// The MCP handler.
#[derive(Clone)]
pub struct CantorMcp {
    engine: Arc<Engine>,
    tool_router: ToolRouter<Self>,
}

impl std::fmt::Debug for CantorMcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CantorMcp").finish_non_exhaustive()
    }
}

/// A tool failure agents can act on.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ToolError {
    /// `not_found`, `invalid`, `rejected`, `revision_conflict`, `io`, `empty`.
    pub code: String,
    /// What went wrong.
    pub message: String,
    /// Details (diagnostics).
    pub diagnostics: Value,
}

impl From<EngineError> for ToolError {
    fn from(e: EngineError) -> Self {
        Self {
            code: e.code.to_owned(),
            message: e.message,
            diagnostics: json!(e.diagnostics),
        }
    }
}

type ToolResult = Result<Json<Value>, Json<ToolError>>;

async fn blocking<T: Serialize + Send + 'static>(
    f: impl FnOnce() -> crate::engine::Result<T> + Send + 'static,
) -> ToolResult {
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(v)) => Ok(Json(serde_json::to_value(v).unwrap_or(Value::Null))),
        Ok(Err(e)) => Err(Json(e.into())),
        Err(e) => Err(Json(ToolError {
            code: "internal".into(),
            message: e.to_string(),
            diagnostics: Value::Null,
        })),
    }
}

/// A file path.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct PathArgs {
    /// Absolute path.
    pub path: String,
}

/// Optional file path.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SaveArgs {
    /// Where to save (default: where it was opened or last saved).
    #[serde(default)]
    pub path: Option<String>,
}

/// Import arguments.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ImportArgs {
    /// Absolute path of a .mid, .ust, .ustx, .musicxml, .cantor, lyrics .txt or audio .wav file.
    pub path: String,
    /// midi, ust, ustx, music_xml, cantor, lyrics or audio (default: from the extension).
    #[serde(default)]
    pub kind: Option<String>,
}

/// A voicebank id.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct VoicebankArgs {
    /// Id from list_voicebanks.
    pub id: String,
}

/// get_notes arguments.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct NotesArgs {
    /// Track id (default: all tracks).
    #[serde(default)]
    pub track: Option<u64>,
    /// First tick (480 per quarter note).
    #[serde(default)]
    pub from: Option<i64>,
    /// End tick.
    #[serde(default)]
    pub to: Option<i64>,
}

/// apply_ops arguments.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct OpsArgs {
    /// Ops, applied atomically as one undo step (wrap related ones in a labeled `batch`). See the `schema` tool.
    pub ops: Vec<Op>,
    /// The revision your edit is based on; a stale one is rejected so you never overwrite the person's edits.
    #[serde(default)]
    pub base_revision: Option<Revision>,
}

/// undo/redo arguments.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct StepsArgs {
    /// Steps (default 1).
    #[serde(default)]
    pub steps: Option<usize>,
}

/// A checkpoint name.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct NameArgs {
    /// Checkpoint name.
    pub name: String,
}

/// set_lyrics arguments.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct LyricsArgs {
    /// Track id.
    pub track: u64,
    /// Lyrics text: kana, romaji or space-separated syllables; fills notes in order (skipping `+` and rests).
    pub text: String,
    /// Only notes starting in [from, to) ticks.
    #[serde(default)]
    pub from: Option<i64>,
    /// End tick.
    #[serde(default)]
    pub to: Option<i64>,
}

/// render arguments.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RenderArgs {
    /// Only these track ids (default: all audible tracks).
    #[serde(default)]
    pub tracks: Option<Vec<u64>>,
}

/// export arguments.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExportArgs {
    /// wav, ust, ustx or cantor.
    pub format: String,
    /// Absolute output path.
    pub path: String,
}

#[tool_router]
impl CantorMcp {
    /// A handler over a shared engine.
    pub fn new(engine: Arc<Engine>) -> Self {
        Self {
            engine,
            tool_router: Self::tool_router(),
        }
    }

    /// The open project (meta, tempo, tracks with note counts), its revision, path and undo state. Read `cantor://guide` first.
    #[tool(annotations(read_only_hint = true))]
    async fn project_info(&self) -> ToolResult {
        let e = self.engine.clone();
        blocking(move || {
            let s = e.state();
            Ok(json!({
                "revision": s.revision, "path": s.path, "dirty": s.dirty, "can_undo": s.can_undo, "can_redo": s.can_redo,
                "meta": s.project.meta, "tempo": s.project.tempo, "time_sig": s.project.time_sig, "key": s.project.key,
                "sections": s.project.sections, "reference": s.project.reference,
                "tracks": s.project.tracks.iter().map(|t| json!({ "id": t.id, "name": t.name, "singer": t.singer,
                    "phonemizer": t.phonemizer, "flags": t.flags, "mix": t.mix, "notes": t.notes.len(),
                    "curves": t.curves.keys().collect::<Vec<_>>() })).collect::<Vec<_>>(),
            }))
        })
        .await
    }

    /// Opens a .cantor project (replaces the open one and its history).
    #[tool]
    async fn open_project(&self, Parameters(a): Parameters<PathArgs>) -> ToolResult {
        let e = self.engine.clone();
        blocking(move || e.open(std::path::Path::new(&a.path), &Caller::mcp())).await
    }

    /// Starts an empty project with one track on the first voicebank.
    #[tool]
    async fn new_project(&self) -> ToolResult {
        let e = self.engine.clone();
        blocking(move || {
            e.new_project(&Caller::mcp())
                .map(|s| json!({ "revision": s.revision }))
        })
        .await
    }

    /// Saves the project as .cantor.
    #[tool]
    async fn save_project(&self, Parameters(a): Parameters<SaveArgs>) -> ToolResult {
        let e = self.engine.clone();
        blocking(move || {
            e.save(a.path.as_deref().map(std::path::Path::new))
                .map(|p| json!({ "path": p }))
        })
        .await
    }

    /// Imports a file: a score (MIDI/UST/USTX/MusicXML/.cantor) replaces the project as one undoable step; lyrics text fills the first track's notes; audio becomes the reference vocal.
    #[tool]
    async fn import(&self, Parameters(a): Parameters<ImportArgs>) -> ToolResult {
        let e = self.engine.clone();
        blocking(move || {
            e.import(
                std::path::Path::new(&a.path),
                a.kind.as_deref(),
                None,
                &Caller::mcp(),
            )
        })
        .await
    }

    /// Installed voicebanks (id, name, alias count, kind cv/vcv/cvvc/mixed).
    #[tool(annotations(read_only_hint = true))]
    async fn list_voicebanks(&self) -> ToolResult {
        let e = self.engine.clone();
        blocking(move || Ok(json!({ "voicebanks": e.voicebanks() }))).await
    }

    /// A voicebank's aliases, prefix map and load warnings.
    #[tool(annotations(read_only_hint = true))]
    async fn voicebank_info(&self, Parameters(a): Parameters<VoicebankArgs>) -> ToolResult {
        let e = self.engine.clone();
        blocking(move || e.voicebank_info(&a.id)).await
    }

    /// Notes as a compact table [id, start, length, key, lyric, sec] per track, optionally in a tick range.
    #[tool(annotations(read_only_hint = true))]
    async fn get_notes(&self, Parameters(a): Parameters<NotesArgs>) -> ToolResult {
        let e = self.engine.clone();
        blocking(move || {
            e.notes(a.track.map(TrackId), a.from.zip(a.to))
                .map(|t| json!({ "tracks": t }))
        })
        .await
    }

    /// Applies ops atomically as one undo step (see the `schema` tool for every op). Edits show up live in the person's browser.
    #[tool]
    async fn apply_ops(&self, Parameters(a): Parameters<OpsArgs>) -> ToolResult {
        let e = self.engine.clone();
        blocking(move || e.apply_ops(a.ops, a.base_revision, &Caller::mcp())).await
    }

    /// Undoes steps.
    #[tool]
    async fn undo(&self, Parameters(a): Parameters<StepsArgs>) -> ToolResult {
        let e = self.engine.clone();
        blocking(move || e.undo_redo(false, a.steps.unwrap_or(1), &Caller::mcp())).await
    }

    /// Redoes steps.
    #[tool]
    async fn redo(&self, Parameters(a): Parameters<StepsArgs>) -> ToolResult {
        let e = self.engine.clone();
        blocking(move || e.undo_redo(true, a.steps.unwrap_or(1), &Caller::mcp())).await
    }

    /// Undo/redo labels (with who made each step) and checkpoint names.
    #[tool(annotations(read_only_hint = true))]
    async fn history(&self) -> ToolResult {
        let e = self.engine.clone();
        blocking(move || Ok(e.history())).await
    }

    /// Saves a named snapshot of the project (do this before risky passes).
    #[tool]
    async fn checkpoint(&self, Parameters(a): Parameters<NameArgs>) -> ToolResult {
        let e = self.engine.clone();
        blocking(move || e.checkpoint(&a.name, &Caller::mcp())).await
    }

    /// Returns to a checkpoint, as one undoable step.
    #[tool]
    async fn restore(&self, Parameters(a): Parameters<NameArgs>) -> ToolResult {
        let e = self.engine.clone();
        blocking(move || e.restore(&a.name, &Caller::mcp())).await
    }

    /// Runs a tuner pass: {pass: auto_pitch|auto_dynamics|breaths|section_color|match_reference|humanize, track, range?: [from, to) ticks, params}. Emits ordinary undoable ops. See docs in cantor://guide and the `schema` tool.
    #[tool]
    async fn tune(&self, Parameters(a): Parameters<TuneRequest>) -> ToolResult {
        let e = self.engine.clone();
        blocking(move || e.tune(a, &Caller::mcp())).await
    }

    /// Puts lyrics on a track's notes in order.
    #[tool]
    async fn set_lyrics(&self, Parameters(a): Parameters<LyricsArgs>) -> ToolResult {
        let e = self.engine.clone();
        blocking(move || {
            let p = e.project();
            let t = p.track(TrackId(a.track)).ok_or_else(|| EngineError {
                code: "not_found",
                message: format!("no track {}", a.track),
                diagnostics: Vec::new(),
            })?;
            let syl = cantor_io::lyrics::parse(&a.text, Default::default());
            let (op, mut diags) = cantor_io::lyrics::assign(t, &syl, a.from.zip(a.to));
            let mut r = e.apply_ops(
                vec![Op::batch("Set lyrics", vec![op])],
                None,
                &Caller::mcp(),
            )?;
            diags.append(&mut r.diagnostics);
            r.diagnostics = diags;
            Ok(r)
        })
        .await
    }

    /// Analyzes the reference vocal: F0 summary (voiced share, range) to check it's usable. The UI draws it over the piano roll.
    #[tool(annotations(read_only_hint = true))]
    async fn analyze_reference(&self) -> ToolResult {
        let e = self.engine.clone();
        blocking(move || {
            let v = e.reference_f0_view()?;
            let cents: Vec<f64> = v["cents"].as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
            let total = v["cents"].as_array().map_or(0, Vec::len);
            let (lo, hi) = cents.iter().fold((f64::MAX, f64::MIN), |(a, b), c| (a.min(*c), b.max(*c)));
            Ok(json!({ "frames": total, "step_ms": v["step_ms"], "voiced_ratio": if total > 0 { cents.len() as f64 / total as f64 } else { 0.0 },
                "range_cents": if cents.is_empty() { Value::Null } else { json!([lo, hi]) } }))
        })
        .await
    }

    /// Transcribes the reference vocal into notes (no lyrics) on a new track.
    #[tool]
    async fn transcribe(&self) -> ToolResult {
        let e = self.engine.clone();
        blocking(move || e.transcribe(&Caller::mcp())).await
    }

    /// Renders and returns the report: pitch error vs score and vs reference (p50/p95/bias), onset error, LUFS, true peak, clipping, missing aliases, per-phrase results with the worst note, artifacts. Plus the WAV path.
    #[tool]
    async fn render(&self, Parameters(a): Parameters<RenderArgs>) -> ToolResult {
        let e = self.engine.clone();
        blocking(move || {
            e.render(
                a.tracks.map(|v| v.into_iter().map(TrackId).collect()),
                &Caller::mcp(),
            )
        })
        .await
    }

    /// Writes the project as wav, ust, ustx or cantor.
    #[tool]
    async fn export(&self, Parameters(a): Parameters<ExportArgs>) -> ToolResult {
        let e = self.engine.clone();
        blocking(move || {
            let (bytes, _, _, diags) = e.export(&a.format)?;
            std::fs::write(&a.path, bytes).map_err(|err| EngineError {
                code: "io",
                message: err.to_string(),
                diagnostics: Vec::new(),
            })?;
            Ok(json!({ "path": a.path, "diagnostics": diags }))
        })
        .await
    }

    /// JSON schemas: ops, tune requests, curve/note parameters (unit, range, default) and effects.
    #[tool(annotations(read_only_hint = true))]
    async fn schema(&self) -> ToolResult {
        let e = self.engine.clone();
        blocking(move || Ok(e.schema())).await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for CantorMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().enable_resources().build())
            .with_server_info(Implementation::new("cantor", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Cantor makes UTAU covers. Read the resource cantor://guide first. Edit with apply_ops/tune (they \
                 appear live in the person's browser editor), render to get numbers (you cannot hear), and report the \
                 numbers. Ticks: 480 per quarter note.",
            )
    }

    async fn list_resources(
        &self,
        _r: Option<PaginatedRequestParams>,
        _cx: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult {
            resources: vec![
                Resource::new(GUIDE_URI, "How to make a cover with Cantor")
                    .with_mime_type("text/markdown"),
            ],
            ..Default::default()
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _cx: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        if request.uri != GUIDE_URI {
            return Err(ErrorData::invalid_params(
                format!("resource not found: {}", request.uri),
                None,
            ));
        }
        Ok(
            ReadResourceResult::new(vec![ResourceContents::TextResourceContents {
                uri: request.uri,
                mime_type: Some("text/markdown".into()),
                text: GUIDE.into(),
                meta: None,
            }])
            .into(),
        )
    }
}

/// Serves MCP over stdio until the client disconnects.
pub async fn serve_stdio(engine: Arc<Engine>) -> Result<(), Box<dyn std::error::Error>> {
    let running = CantorMcp::new(engine)
        .serve(rmcp::transport::stdio())
        .await?;
    running.waiting().await?;
    Ok(())
}

/// `/mcp` (streamable HTTP), loopback only.
pub fn router(engine: Arc<Engine>) -> axum::Router {
    let config = StreamableHttpServerConfig::default()
        .with_allowed_hosts(["localhost", "127.0.0.1", "::1"])
        .with_allowed_origins(["http://localhost:*", "http://127.0.0.1:*", "http://[::1]:*"])
        .enforce_origin_validation();
    let service = StreamableHttpService::new(
        move || Ok(CantorMcp::new(engine.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    axum::Router::new().nest_service("/mcp", service)
}
