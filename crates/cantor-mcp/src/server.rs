//! Cantor's MCP server (protocol revision 2026-07-28, stateless).
//!
//! There are no sessions: every call names its `project_id`, and edits
//! carry `base_revision`; a stale revision returns a `conflict` error with
//! the diff since that revision. Tools map one-to-one onto
//! `cantor_server::Engine` methods.

use cantor_core::{Op, PhraseId, RetakeDim, Revision, TakeId, TickRange};
use cantor_render::render::Quality;
use cantor_server::describe::Detail;
use cantor_server::{Engine, EngineError, TrackRef};
use rmcp::handler::server::router::prompt::PromptRouter;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::*;
use rmcp::service::RequestContext;
use rmcp::{
    ErrorData, RoleServer, ServerHandler, prompt, prompt_handler, prompt_router, tool,
    tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

/// Cache lifetime advertised for list and resource results.
const TTL_MS: u64 = 1_000;

/// The MCP server state: an engine shared by all requests.
#[derive(Clone)]
pub struct CantorMcp {
    engine: Arc<Engine>,
    tool_router: ToolRouter<Self>,
    prompt_router: PromptRouter<Self>,
}

impl std::fmt::Debug for CantorMcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CantorMcp").finish_non_exhaustive()
    }
}

/// A tool failure, returned as an `isError` result agents can act on.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ToolError {
    /// `not_found`, `conflict`, `invalid`, `io` or `render`.
    pub code: String,
    /// What went wrong.
    pub message: String,
    /// For conflicts: the current revision to retry against.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_revision: Option<Revision>,
    /// For conflicts: what changed since `base_revision`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<Box<cantor_core::DocDiff>>,
}

impl From<EngineError> for ToolError {
    fn from(e: EngineError) -> Self {
        let (current_revision, diff) = match &e {
            EngineError::Conflict { current, diff, .. } => {
                (Some(*current), Some(Box::new(diff.clone())))
            }
            _ => (None, None),
        };
        Self {
            code: e.code().to_owned(),
            message: e.to_string(),
            current_revision,
            diff,
        }
    }
}

type ToolResult<T> = Result<Json<T>, Json<ToolError>>;

/// Runs a blocking engine call off the async runtime.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, EngineError> + Send + 'static,
) -> Result<T, Json<ToolError>> {
    match tokio::task::spawn_blocking(f).await {
        Ok(r) => r.map_err(|e| Json(ToolError::from(e))),
        Err(e) => Err(Json(ToolError {
            code: "internal".into(),
            message: e.to_string(),
            current_revision: None,
            diff: None,
        })),
    }
}

/// A text answer wrapped for structured output.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Text {
    /// The text.
    pub text: String,
}

/// Arguments naming a file.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct PathArgs {
    /// Absolute path.
    pub path: PathBuf,
}

/// Arguments naming a project.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProjectArgs {
    /// Handle from open_project, new_project or import_file.
    pub project_id: String,
}

/// Arguments for new_project.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct NewProjectArgs {
    /// Song title.
    #[serde(default)]
    pub title: String,
}

/// Arguments for describe_range.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DescribeRangeArgs {
    /// Project handle.
    pub project_id: String,
    /// Track id or name; all tracks when omitted.
    #[serde(default)]
    pub track: Option<TrackRef>,
    /// First tick (1920 ticks per quarter note).
    pub t0: i64,
    /// Tick after the range.
    pub t1: i64,
    /// notes, curves (default) or full (every curve point).
    #[serde(default)]
    pub detail: Detail,
}

/// Arguments for get_singer_schema.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SingerArgs {
    /// Singer id from list_singers.
    pub singer: String,
}

/// Arguments for apply_ops.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ApplyOpsArgs {
    /// Project handle.
    pub project_id: String,
    /// The revision your edit is based on (from the last result).
    pub base_revision: Revision,
    /// Ops, applied atomically as one undo step. See the op schema resource.
    pub ops: Vec<Op>,
    /// Report what would change without changing anything.
    #[serde(default)]
    pub dry_run: bool,
}

/// Arguments for retake.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RetakeArgs {
    /// Project handle.
    pub project_id: String,
    /// Base revision.
    pub base_revision: Revision,
    /// Track id or name.
    pub track: TrackRef,
    /// Tick range; phrases overlapping it are retaken.
    pub range: TickRange,
    /// pitch, timing, timbre, expression.
    pub dims: Vec<RetakeDim>,
    /// Takes per phrase (1–16).
    pub n_variants: u8,
    /// Base seed for reproducible takes.
    #[serde(default)]
    pub seed: Option<u64>,
}

/// Arguments for select_take.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SelectTakeArgs {
    /// Project handle.
    pub project_id: String,
    /// Base revision.
    pub base_revision: Revision,
    /// Phrase id (its first note's id).
    pub phrase_id: u64,
    /// Take id; omit for the original.
    #[serde(default)]
    pub take_id: Option<u64>,
}

/// Arguments for render.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RenderArgs {
    /// Project handle.
    pub project_id: String,
    /// Render only phrases overlapping this tick range.
    #[serde(default)]
    pub range: Option<TickRange>,
    /// preview (fast) or final.
    #[serde(default = "preview")]
    pub quality: Quality,
}

fn preview() -> Quality {
    Quality::Preview
}

/// Arguments for compare.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CompareArgs {
    /// Baseline render id.
    pub render_a: String,
    /// Compared render id.
    pub render_b: String,
}

/// Arguments for checkpoint.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckpointArgs {
    /// Project handle.
    pub project_id: String,
    /// Checkpoint name.
    pub name: String,
}

/// Arguments for restore.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RestoreArgs {
    /// Project handle.
    pub project_id: String,
    /// Base revision.
    pub base_revision: Revision,
    /// Checkpoint name, or `#<node>` from list_history.
    pub name: String,
}

/// Arguments for export and save.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExportArgs {
    /// Project handle.
    pub project_id: String,
    /// cantor (project file) or wav (final render).
    pub format: String,
    /// Absolute output path.
    pub path: PathBuf,
}

/// Wraps a list for structured output (results must be objects).
#[derive(Debug, Serialize, JsonSchema)]
pub struct List<T> {
    /// The items.
    pub items: Vec<T>,
}

/// A path result.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Written {
    /// The file written.
    pub path: String,
}

#[tool_router]
impl CantorMcp {
    /// Opens a .cantor project file. Returns project_id and revision.
    #[tool(annotations(read_only_hint = true, open_world_hint = false))]
    async fn open_project(
        &self,
        Parameters(a): Parameters<PathArgs>,
    ) -> ToolResult<cantor_server::engine::ProjectInfo> {
        let e = self.engine.clone();
        blocking(move || e.open_project(&a.path)).await.map(Json)
    }

    /// Creates an empty project (120 BPM, 4/4).
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = false,
        open_world_hint = false
    ))]
    async fn new_project(
        &self,
        Parameters(a): Parameters<NewProjectArgs>,
    ) -> ToolResult<cantor_server::engine::ProjectInfo> {
        Ok(Json(self.engine.new_project(&a.title)))
    }

    /// Imports USTX, MIDI or MusicXML as a new project; diagnostics list what was approximated.
    #[tool(annotations(read_only_hint = true, open_world_hint = false))]
    async fn import_file(
        &self,
        Parameters(a): Parameters<PathArgs>,
    ) -> ToolResult<cantor_server::engine::ProjectInfo> {
        let e = self.engine.clone();
        blocking(move || e.import_file(&a.path)).await.map(Json)
    }

    /// Compact text overview: tempo, sections, tracks, phrases, notes with lyrics, phonemes and timing. Start here.
    #[tool(annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false))]
    async fn describe_project(&self, Parameters(a): Parameters<ProjectArgs>) -> ToolResult<Text> {
        let e = self.engine.clone();
        blocking(move || e.describe_project(&a.project_id))
            .await
            .map(|text| Json(Text { text }))
    }

    /// A tick range of the project; curves summarized as key points and statistics (detail=full lists every point).
    #[tool(annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false))]
    async fn describe_range(
        &self,
        Parameters(a): Parameters<DescribeRangeArgs>,
    ) -> ToolResult<Text> {
        let e = self.engine.clone();
        blocking(move || {
            e.describe_range(
                &a.project_id,
                a.track.as_ref(),
                TickRange {
                    start: a.t0,
                    end: a.t1,
                },
                a.detail,
            )
        })
        .await
        .map(|text| Json(Text { text }))
    }

    /// Installed singers with languages and licence terms.
    #[tool(annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false))]
    async fn list_singers(&self) -> ToolResult<List<cantor_server::singers::SingerInfo>> {
        let e = self.engine.clone();
        blocking(move || Ok(e.list_singers()))
            .await
            .map(|items| Json(List { items }))
    }

    /// Installed phonemizer plugins (WASM) and the languages they handle, plus plugins that failed to load. A plugin replaces the voicebank dictionary for its languages.
    #[tool(annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false))]
    async fn list_phonemizers(&self) -> ToolResult<cantor_server::engine::PhonemizerList> {
        let e = self.engine.clone();
        blocking(move || Ok(e.list_phonemizers())).await.map(Json)
    }

    /// A singer's parameters (unit, range, default, supported), retake dimensions, X-SAMPA spellings and licence.
    #[tool(annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false))]
    async fn get_singer_schema(
        &self,
        Parameters(a): Parameters<SingerArgs>,
    ) -> ToolResult<cantor_server::singers::SingerSchema> {
        let e = self.engine.clone();
        blocking(move || e.get_singer_schema(&a.singer))
            .await
            .map(Json)
    }

    /// Applies ops atomically. Pass the latest revision as base_revision; a stale one returns a conflict with the diff. Use dry_run to preview.
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = false,
        open_world_hint = false
    ))]
    async fn apply_ops(
        &self,
        Parameters(a): Parameters<ApplyOpsArgs>,
    ) -> ToolResult<cantor_core::ApplyResult> {
        let e = self.engine.clone();
        blocking(move || e.apply_ops(&a.project_id, a.base_revision, a.ops, a.dry_run))
            .await
            .map(Json)
    }

    /// Generates n_variants new takes for phrases overlapping range, re-rolling the chosen dims; selects the first new take.
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = true,
        open_world_hint = false
    ))]
    async fn retake(
        &self,
        Parameters(a): Parameters<RetakeArgs>,
    ) -> ToolResult<cantor_core::ApplyResult> {
        let e = self.engine.clone();
        blocking(move || {
            e.retake(
                &a.project_id,
                a.base_revision,
                &a.track,
                a.range,
                a.dims,
                a.n_variants,
                a.seed,
            )
        })
        .await
        .map(Json)
    }

    /// Chooses which take of a phrase plays (omit take_id for the original).
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    async fn select_take(
        &self,
        Parameters(a): Parameters<SelectTakeArgs>,
    ) -> ToolResult<cantor_core::ApplyResult> {
        let e = self.engine.clone();
        blocking(move || {
            e.select_take(
                &a.project_id,
                a.base_revision,
                PhraseId(a.phrase_id),
                a.take_id.map(TakeId),
            )
        })
        .await
        .map(Json)
    }

    /// Renders (optionally only phrases in range) and returns the audio URI plus the analysis report: per-note pitch error (with error_contour to correct), timing, loudness, artifacts.
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    async fn render(
        &self,
        Parameters(a): Parameters<RenderArgs>,
    ) -> ToolResult<cantor_server::engine::RenderSummary> {
        let e = self.engine.clone();
        blocking(move || e.render(&a.project_id, a.range, a.quality))
            .await
            .map(Json)
    }

    /// Metric deltas (b − a) between two renders, including per-note p95 pitch error.
    #[tool(annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false))]
    async fn compare(
        &self,
        Parameters(a): Parameters<CompareArgs>,
    ) -> ToolResult<cantor_server::engine::Comparison> {
        let e = self.engine.clone();
        blocking(move || e.compare(&a.render_a, &a.render_b))
            .await
            .map(Json)
    }

    /// Names the current state so restore can return to it.
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    async fn checkpoint(
        &self,
        Parameters(a): Parameters<CheckpointArgs>,
    ) -> ToolResult<cantor_server::engine::HistoryEntry> {
        let e = self.engine.clone();
        blocking(move || e.checkpoint(&a.project_id, &a.name))
            .await
            .map(Json)
    }

    /// Returns to a checkpoint (or history node `#<n>`); later edits stay on their branch.
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = true,
        open_world_hint = false
    ))]
    async fn restore(
        &self,
        Parameters(a): Parameters<RestoreArgs>,
    ) -> ToolResult<cantor_core::ApplyResult> {
        let e = self.engine.clone();
        blocking(move || e.restore(&a.project_id, a.base_revision, &a.name))
            .await
            .map(Json)
    }

    /// The undo tree: every edit with its revision, branches and checkpoints.
    #[tool(annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false))]
    async fn list_history(
        &self,
        Parameters(a): Parameters<ProjectArgs>,
    ) -> ToolResult<List<cantor_server::engine::HistoryEntry>> {
        let e = self.engine.clone();
        blocking(move || e.list_history(&a.project_id))
            .await
            .map(|items| Json(List { items }))
    }

    /// Writes the project (format cantor) or a final-quality render (format wav) to path.
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = true,
        open_world_hint = false
    ))]
    async fn export(&self, Parameters(a): Parameters<ExportArgs>) -> ToolResult<Written> {
        let e = self.engine.clone();
        blocking(move || e.export(&a.project_id, &a.format, &a.path))
            .await
            .map(|p| {
                Json(Written {
                    path: p.display().to_string(),
                })
            })
    }
}

/// Arguments for the tuning prompt.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct TunePromptArgs {
    /// Project handle.
    pub project_id: String,
    /// Phrase id to tune.
    pub phrase_id: String,
}

/// Arguments for prompts that take a project.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProjectPromptArgs {
    /// Project handle.
    pub project_id: String,
}

fn user(text: String) -> Vec<PromptMessage> {
    vec![PromptMessage::new_text(Role::User, text)]
}

#[prompt_router]
impl CantorMcp {
    /// Tune a phrase to at most 15 cents p95 pitch error.
    #[prompt(name = "tune_phrase")]
    async fn tune_phrase(&self, Parameters(a): Parameters<TunePromptArgs>) -> Vec<PromptMessage> {
        user(format!(
            "Tune phrase {} of project {} to ≤15 cents p95 pitch error. Loop: render with range set to the \
             phrase; read each note's pitch_error_p95 and error_contour; apply_ops one SetCurve per note on \
             param \"pitch\" whose points subtract the measured error (add to any earlier corrections, with a \
             0-valued point just outside each note's steady part); re-render (pitch-only edits use the fast \
             vocoder path) and stop when the phrase's p95 is ≤15 cents. Always pass the latest revision.",
            a.phrase_id, a.project_id
        ))
    }

    /// Make the chorus breathier and more intimate.
    #[prompt(name = "breathier_chorus")]
    async fn breathier_chorus(
        &self,
        Parameters(a): Parameters<ProjectPromptArgs>,
    ) -> Vec<PromptMessage> {
        user(format!(
            "Make the chorus of project {} breathier and more intimate: find the chorus section with \
             describe_project, check get_singer_schema for breathiness, energy and tension support, raise \
             breathiness and lower energy/tension with SetCurve over the chorus (ramping in), soften vibrato, \
             then render and compare against a render made before the edit.",
            a.project_id
        ))
    }

    /// Add a two-part harmony a third above.
    #[prompt(name = "add_harmony")]
    async fn add_harmony(
        &self,
        Parameters(a): Parameters<ProjectPromptArgs>,
    ) -> Vec<PromptMessage> {
        user(format!(
            "Add a two-part harmony a diatonic third above the lead of project {}: read the key map and \
             notes with describe_project, add a track (AddTrack) with the same singer, copy each lead note \
             raised by a major or minor third so it stays in key, keep lyrics and timing, lower the harmony \
             track's gain by 4 dB, and render to check pitch accuracy of both tracks.",
            a.project_id
        ))
    }
}

impl CantorMcp {
    /// A server over a shared engine.
    pub fn new(engine: Arc<Engine>) -> Self {
        Self {
            engine,
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    fn resources(&self) -> Vec<Resource> {
        let mut out = Vec::new();
        for entry in cantor_core::schema::registry() {
            out.push(
                Resource::new(
                    format!("cantor://schema/{}", entry.name),
                    format!("{} JSON Schema", entry.name),
                )
                .with_mime_type("application/schema+json"),
            );
        }
        out.push(
            Resource::new("cantor://schema/params", "parameter registry")
                .with_mime_type("application/json"),
        );
        out.push(
            Resource::new("cantor://schema/effects", "effect registry")
                .with_mime_type("application/json"),
        );
        for s in self.engine.list_singers() {
            out.push(
                Resource::new(
                    format!("cantor://singers/{}", s.id),
                    format!("singer {}", s.name),
                )
                .with_mime_type("application/json"),
            );
        }
        out
    }

    fn read(&self, uri: &str) -> Option<(String, String)> {
        let json = |v: serde_json::Value| {
            Some((
                serde_json::to_string_pretty(&v).ok()?,
                "application/json".to_owned(),
            ))
        };
        if let Some(name) = uri.strip_prefix("cantor://schema/") {
            if name == "params" {
                return json(serde_json::to_value(cantor_core::params::registry()).ok()?);
            }
            if name == "effects" {
                return json(serde_json::to_value(cantor_core::effects::registry()).ok()?);
            }
            let entry = cantor_core::schema::registry()
                .into_iter()
                .find(|e| e.name == name)?;
            return json(serde_json::to_value(entry.schema).ok()?);
        }
        if let Some(id) = uri.strip_prefix("cantor://projects/") {
            let (p, rev) = self.engine.snapshot(id).ok()?;
            return json(serde_json::json!({ "revision": rev, "project": *p }));
        }
        if let Some(id) = uri.strip_prefix("cantor://renders/") {
            return json(serde_json::to_value(self.engine.get_render(id).ok()?).ok()?);
        }
        if let Some(id) = uri.strip_prefix("cantor://singers/") {
            return json(serde_json::to_value(self.engine.get_singer_schema(id).ok()?).ok()?);
        }
        None
    }
}

#[tool_handler(router = self.tool_router)]
#[prompt_handler(router = self.prompt_router)]
impl ServerHandler for CantorMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new("cantor", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "Cantor edits and renders sung vocals. Open or import a project, read it with describe_project, \
             edit with apply_ops (always pass the latest revision), render to hear-by-numbers: every render \
             returns pitch, timing, loudness and artifact measurements. Ticks: 1920 per quarter note.",
        )
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&[ProtocolVersion::V_2026_07_28])
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _cx: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult {
            resources: self.resources(),
            ..Default::default()
        }
        .with_ttl_ms(TTL_MS)
        .with_cache_scope(CacheScope::Private))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _cx: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        let t = |uri: &str, name: &str| {
            ResourceTemplate::new(uri, name).with_mime_type("application/json")
        };
        Ok(ListResourceTemplatesResult {
            resource_templates: vec![
                t(
                    "cantor://projects/{project_id}",
                    "project JSON with revision",
                ),
                t(
                    "cantor://renders/{render_id}",
                    "render report with analysis",
                ),
                t("cantor://singers/{singer_id}", "singer schema and licence"),
            ],
            ..Default::default()
        }
        .with_ttl_ms(TTL_MS)
        .with_cache_scope(CacheScope::Private))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _cx: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let uri = request.uri.clone();
        let this = self.clone();
        let found = tokio::task::spawn_blocking(move || this.read(&uri))
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        match found {
            Some((text, mime)) => {
                Ok(
                    ReadResourceResult::new(vec![ResourceContents::TextResourceContents {
                        uri: request.uri,
                        mime_type: Some(mime),
                        text,
                        meta: None,
                    }])
                    .with_ttl_ms(TTL_MS)
                    .with_cache_scope(CacheScope::Private)
                    .into(),
                )
            }
            None => Err(ErrorData::invalid_params(
                format!("resource not found: {}", request.uri),
                None,
            )),
        }
    }
}
