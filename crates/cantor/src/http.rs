//! The HTTP JSON API and the embedded web editor (docs/design.md "HTTP API").
//! Requests may carry `X-Cantor-Client`; events echo it.

use crate::engine::{Caller, Engine, EngineError, Source};
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use cantor_core::{Op, Revision, TrackId, TuneRequest};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

type Shared = Arc<Engine>;

/// The web editor's files, embedded at compile time (no build step).
pub const WEB: &[(&str, &str, &str)] = &[
    (
        "index.html",
        "text/html; charset=utf-8",
        include_str!("../../../web/index.html"),
    ),
    (
        "style.css",
        "text/css; charset=utf-8",
        include_str!("../../../web/style.css"),
    ),
    ("js/api.js", JS, include_str!("../../../web/js/api.js")),
    (
        "js/connect.js",
        JS,
        include_str!("../../../web/js/connect.js"),
    ),
    ("js/edit.js", JS, include_str!("../../../web/js/edit.js")),
    (
        "js/history.js",
        JS,
        include_str!("../../../web/js/history.js"),
    ),
    ("js/lanes.js", JS, include_str!("../../../web/js/lanes.js")),
    ("js/main.js", JS, include_str!("../../../web/js/main.js")),
    ("js/mock.js", JS, include_str!("../../../web/js/mock.js")),
    (
        "js/pianoroll.js",
        JS,
        include_str!("../../../web/js/pianoroll.js"),
    ),
    ("js/props.js", JS, include_str!("../../../web/js/props.js")),
    (
        "js/report.js",
        JS,
        include_str!("../../../web/js/report.js"),
    ),
    ("js/state.js", JS, include_str!("../../../web/js/state.js")),
    (
        "js/tracks.js",
        JS,
        include_str!("../../../web/js/tracks.js"),
    ),
    (
        "js/transport.js",
        JS,
        include_str!("../../../web/js/transport.js"),
    ),
    ("js/ui.js", JS, include_str!("../../../web/js/ui.js")),
];
const JS: &str = "text/javascript; charset=utf-8";

struct ApiError(EngineError);

impl From<EngineError> for ApiError {
    fn from(e: EngineError) -> Self {
        Self(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.0.code {
            "not_found" => StatusCode::NOT_FOUND,
            "revision_conflict" => StatusCode::CONFLICT,
            "io" | "internal" => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::UNPROCESSABLE_ENTITY,
        };
        let body = json!({ "error": { "code": self.0.code, "message": self.0.message, "diagnostics": self.0.diagnostics } });
        (status, Json(body)).into_response()
    }
}

type ApiResult = Result<Response, ApiError>;

fn caller(h: &HeaderMap) -> Caller {
    Caller {
        source: Source::Http,
        client: h
            .get("x-cantor-client")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned),
    }
}

/// Runs a blocking engine call and serializes its result.
async fn run<T: serde::Serialize + Send + 'static>(
    f: impl FnOnce() -> crate::engine::Result<T> + Send + 'static,
) -> ApiResult {
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(v)) => Ok(Json(v).into_response()),
        Ok(Err(e)) => Err(e.into()),
        Err(e) => Err(EngineError {
            code: "internal",
            message: e.to_string(),
            diagnostics: Vec::new(),
        }
        .into()),
    }
}

async fn run_bytes(
    mime: &'static str,
    f: impl FnOnce() -> crate::engine::Result<Vec<u8>> + Send + 'static,
) -> ApiResult {
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(v)) => Ok(([(header::CONTENT_TYPE, mime)], v).into_response()),
        Ok(Err(e)) => Err(e.into()),
        Err(e) => Err(EngineError {
            code: "internal",
            message: e.to_string(),
            diagnostics: Vec::new(),
        }
        .into()),
    }
}

/// The API and web UI router.
pub fn router(engine: Shared) -> Router {
    Router::new()
        .route("/", get(|| async { static_file("index.html") }))
        .route(
            "/{*file}",
            get(|Path(f): Path<String>| async move { static_file(&f) }),
        )
        .route(
            "/api/state",
            get(|State(e): State<Shared>| run(move || Ok(e.state()))),
        )
        .route(
            "/api/history",
            get(|State(e): State<Shared>| run(move || Ok(e.history()))),
        )
        .route(
            "/api/schema",
            get(|State(e): State<Shared>| run(move || Ok(e.schema()))),
        )
        .route(
            "/api/voicebanks",
            get(|State(e): State<Shared>| run(move || Ok(e.voicebanks()))),
        )
        .route(
            "/api/voicebanks/rescan",
            post(|State(e): State<Shared>| {
                run(move || Ok(json!({ "warnings": e.rescan(), "voicebanks": e.voicebanks() })))
            }),
        )
        .route(
            "/api/voicebanks/{id}",
            get(|State(e): State<Shared>, Path(id): Path<String>| {
                run(move || e.voicebank_info(&id))
            }),
        )
        .route("/api/voicebanks/{id}/image", get(voicebank_image))
        .route(
            "/api/tracks/{id}/pitch",
            get(|State(e): State<Shared>, Path(id): Path<u64>| run(move || e.pitch(TrackId(id)))),
        )
        .route(
            "/api/tracks/{id}/phonemes",
            get(|State(e): State<Shared>, Path(id): Path<u64>| {
                run(move || e.phonemes(TrackId(id)))
            }),
        )
        .route("/api/notes", get(notes))
        .route(
            "/api/reference/f0",
            get(|State(e): State<Shared>| run(move || e.reference_f0_view())),
        )
        .route(
            "/api/reference/audio",
            get(|State(e): State<Shared>| run_bytes("audio/wav", move || e.reference_audio())),
        )
        .route("/api/render/{file}", get(render_wav))
        .route("/api/export", get(export))
        .route("/api/events", get(events))
        .route("/api/ops", post(ops))
        .route("/api/undo", post(|s, h, b| undo_redo(s, h, b, false)))
        .route("/api/redo", post(|s, h, b| undo_redo(s, h, b, true)))
        .route("/api/checkpoint", post(checkpoint))
        .route("/api/restore", post(restore))
        .route("/api/tune", post(tune))
        .route("/api/render", post(render))
        .route(
            "/api/transcribe",
            post(|State(e): State<Shared>, h: HeaderMap| run(move || e.transcribe(&caller(&h)))),
        )
        .route(
            "/api/project/new",
            post(|State(e): State<Shared>, h: HeaderMap| run(move || e.new_project(&caller(&h)))),
        )
        .route("/api/project/open", post(open))
        .route("/api/project/save", post(save))
        .route(
            "/api/import",
            post(import).layer(DefaultBodyLimit::max(512 * 1024 * 1024)),
        )
        .with_state(engine)
}

fn static_file(path: &str) -> Response {
    let path = path.trim_start_matches('/').trim_start_matches("web/");
    match WEB.iter().find(|(p, _, _)| *p == path) {
        Some((_, mime, body)) => (
            [
                (header::CONTENT_TYPE, *mime),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            *body,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

async fn voicebank_image(State(e): State<Shared>, Path(id): Path<String>) -> ApiResult {
    match tokio::task::spawn_blocking(move || e.voicebank_image(&id)).await {
        Ok(Ok((bytes, mime))) => Ok(([(header::CONTENT_TYPE, mime)], bytes).into_response()),
        Ok(Err(err)) => Err(err.into()),
        Err(err) => Err(EngineError {
            code: "internal",
            message: err.to_string(),
            diagnostics: Vec::new(),
        }
        .into()),
    }
}

#[derive(Deserialize)]
struct NotesQuery {
    track: Option<u64>,
    from: Option<i64>,
    to: Option<i64>,
}

async fn notes(State(e): State<Shared>, Query(q): Query<NotesQuery>) -> ApiResult {
    let range = q.from.zip(q.to);
    run(move || e.notes(q.track.map(TrackId), range)).await
}

async fn render_wav(State(e): State<Shared>, Path(file): Path<String>) -> ApiResult {
    let id = file.trim_end_matches(".wav").to_owned();
    run_bytes("audio/wav", move || e.render_wav(&id)).await
}

#[derive(Deserialize)]
struct ExportQuery {
    format: String,
}

async fn export(State(e): State<Shared>, Query(q): Query<ExportQuery>) -> ApiResult {
    match tokio::task::spawn_blocking(move || e.export(&q.format)).await {
        Ok(Ok((bytes, ext, mime, _))) => Ok((
            [
                (header::CONTENT_TYPE, mime.to_owned()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"cantor.{ext}\""),
                ),
            ],
            bytes,
        )
            .into_response()),
        Ok(Err(err)) => Err(err.into()),
        Err(err) => Err(EngineError {
            code: "internal",
            message: err.to_string(),
            diagnostics: Vec::new(),
        }
        .into()),
    }
}

async fn events(State(e): State<Shared>) -> impl IntoResponse {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    e.subscribe(tx);
    let stream = tokio_stream::StreamExt::map(
        tokio_stream::wrappers::UnboundedReceiverStream::new(rx),
        |s| Ok::<_, std::convert::Infallible>(Event::default().data(s)),
    );
    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[derive(Deserialize)]
struct OpsBody {
    ops: Vec<Op>,
    #[serde(default)]
    base_revision: Option<Revision>,
}

async fn ops(State(e): State<Shared>, h: HeaderMap, Json(b): Json<OpsBody>) -> ApiResult {
    run(move || e.apply_ops(b.ops, b.base_revision, &caller(&h))).await
}

#[derive(Deserialize, Default)]
struct StepsBody {
    #[serde(default)]
    steps: Option<usize>,
}

async fn undo_redo(State(e): State<Shared>, h: HeaderMap, body: Bytes, redo: bool) -> ApiResult {
    let b: StepsBody = serde_json::from_slice(&body).unwrap_or_default();
    run(move || e.undo_redo(redo, b.steps.unwrap_or(1), &caller(&h))).await
}

#[derive(Deserialize)]
struct NameBody {
    name: String,
}

async fn checkpoint(State(e): State<Shared>, h: HeaderMap, Json(b): Json<NameBody>) -> ApiResult {
    run(move || e.checkpoint(&b.name, &caller(&h))).await
}

async fn restore(State(e): State<Shared>, h: HeaderMap, Json(b): Json<NameBody>) -> ApiResult {
    run(move || e.restore(&b.name, &caller(&h))).await
}

async fn tune(State(e): State<Shared>, h: HeaderMap, Json(b): Json<TuneRequest>) -> ApiResult {
    run(move || e.tune(b, &caller(&h))).await
}

#[derive(Deserialize, Default)]
struct RenderBody {
    #[serde(default)]
    tracks: Option<Vec<TrackId>>,
}

async fn render(State(e): State<Shared>, h: HeaderMap, body: Bytes) -> ApiResult {
    let b: RenderBody = serde_json::from_slice(&body).unwrap_or_default();
    run(move || e.render(b.tracks, &caller(&h))).await
}

#[derive(Deserialize)]
struct PathBody {
    path: String,
    #[serde(default)]
    kind: Option<String>,
}

async fn open(State(e): State<Shared>, h: HeaderMap, Json(b): Json<PathBody>) -> ApiResult {
    run(move || e.open(std::path::Path::new(&b.path), &caller(&h))).await
}

#[derive(Deserialize, Default)]
struct SaveBody {
    #[serde(default)]
    path: Option<String>,
}

async fn save(State(e): State<Shared>, body: Bytes) -> ApiResult {
    let b: SaveBody = serde_json::from_slice(&body).unwrap_or_default();
    run(move || {
        e.save(b.path.as_deref().map(std::path::Path::new))
            .map(|path| json!({ "path": path }))
    })
    .await
}

#[derive(Deserialize)]
struct ImportQuery {
    name: Option<String>,
    kind: Option<String>,
}

/// `{ path, kind? }` as JSON, or the file's bytes with `?name=...&kind=...`.
async fn import(
    State(e): State<Shared>,
    h: HeaderMap,
    Query(q): Query<ImportQuery>,
    body: Bytes,
) -> ApiResult {
    let json_body = h
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    let c = caller(&h);
    if json_body {
        let b: PathBody = serde_json::from_slice(&body).map_err(|err| EngineError {
            code: "invalid",
            message: err.to_string(),
            diagnostics: Vec::new(),
        })?;
        return run(move || e.import(std::path::Path::new(&b.path), b.kind.as_deref(), None, &c))
            .await;
    }
    let name = q.name.unwrap_or_else(|| "upload".into());
    let kind = q.kind;
    let bytes = body.to_vec();
    run(move || {
        e.import(
            std::path::Path::new(&name),
            kind.as_deref(),
            Some(bytes),
            &c,
        )
    })
    .await
}
