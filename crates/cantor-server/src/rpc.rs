//! JSON-RPC 2.0 over stdio (one JSON message per line) and WebSocket.
//!
//! Method names mirror the MCP tools. Mutating calls carry
//! `base_revision`; a stale one fails with code -32002 (`conflict`) and
//! `data: {current_revision, diff}`.

use crate::describe::Detail;
use crate::engine::{Engine, EngineError, TrackRef};
use cantor_core::{Op, PhraseId, RetakeDim, Revision, TakeId, TickRange};
use cantor_render::render::Quality;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;

/// Methods served, for `server/discover`.
pub const METHODS: &[&str] = &[
    "server/discover",
    "open_project",
    "new_project",
    "import_file",
    "save_project",
    "close_project",
    "get_project",
    "describe_project",
    "describe_range",
    "list_singers",
    "get_singer_schema",
    "list_phonemizers",
    "apply_ops",
    "retake",
    "select_take",
    "render",
    "compare",
    "checkpoint",
    "restore",
    "list_history",
    "export",
];

/// A JSON-RPC error.
#[derive(Debug)]
pub struct RpcError {
    code: i64,
    message: String,
    data: Option<Value>,
}

impl RpcError {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    fn to_json(&self) -> Value {
        let mut e = json!({ "code": self.code, "message": self.message });
        if let Some(d) = &self.data {
            e["data"] = d.clone();
        }
        e
    }
}

impl From<EngineError> for RpcError {
    fn from(e: EngineError) -> Self {
        let code = match &e {
            EngineError::NotFound(_) => -32001,
            EngineError::Conflict { .. } => -32002,
            EngineError::Invalid(_) => -32003,
            EngineError::Io(_) => -32004,
            EngineError::Render(_) => -32005,
        };
        let data = match &e {
            EngineError::Conflict { current, diff, .. } => {
                Some(json!({ "code": "conflict", "current_revision": current, "diff": diff }))
            }
            other => Some(json!({ "code": other.code() })),
        };
        Self {
            code,
            message: e.to_string(),
            data,
        }
    }
}

fn params<T: DeserializeOwned>(p: Value) -> Result<T, RpcError> {
    let p = if p.is_null() { json!({}) } else { p };
    serde_json::from_value(p).map_err(|e| RpcError::new(-32602, format!("invalid params: {e}")))
}

fn to_value<T: serde::Serialize>(v: T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(|e| RpcError::new(-32603, e.to_string()))
}

#[derive(Deserialize)]
struct PathP {
    path: PathBuf,
}
#[derive(Deserialize)]
struct ProjectP {
    project_id: String,
}
#[derive(Deserialize)]
struct NewP {
    #[serde(default)]
    title: String,
}
#[derive(Deserialize)]
struct SaveP {
    project_id: String,
    #[serde(default)]
    path: Option<PathBuf>,
}
#[derive(Deserialize)]
struct RangeP {
    project_id: String,
    #[serde(default)]
    track: Option<TrackRef>,
    t0: i64,
    t1: i64,
    #[serde(default)]
    detail: Detail,
}
#[derive(Deserialize)]
struct SingerP {
    singer: String,
}
#[derive(Deserialize)]
struct OpsP {
    project_id: String,
    base_revision: Revision,
    ops: Vec<Op>,
    #[serde(default)]
    dry_run: bool,
}
#[derive(Deserialize)]
struct RetakeP {
    project_id: String,
    base_revision: Revision,
    track: TrackRef,
    range: TickRange,
    dims: Vec<RetakeDim>,
    n_variants: u8,
    #[serde(default)]
    seed: Option<u64>,
}
#[derive(Deserialize)]
struct SelectP {
    project_id: String,
    base_revision: Revision,
    phrase_id: u64,
    #[serde(default)]
    take_id: Option<u64>,
}
#[derive(Deserialize)]
struct RenderP {
    project_id: String,
    #[serde(default)]
    range: Option<TickRange>,
    #[serde(default = "preview")]
    quality: Quality,
}
fn preview() -> Quality {
    Quality::Preview
}
#[derive(Deserialize)]
struct CompareP {
    render_a: String,
    render_b: String,
}
#[derive(Deserialize)]
struct NameP {
    project_id: String,
    name: String,
}
#[derive(Deserialize)]
struct RestoreP {
    project_id: String,
    base_revision: Revision,
    name: String,
}
#[derive(Deserialize)]
struct ExportP {
    project_id: String,
    format: String,
    path: PathBuf,
}

/// Runs one method.
pub fn dispatch(engine: &Engine, method: &str, p: Value) -> Result<Value, RpcError> {
    match method {
        "server/discover" => Ok(json!({
            "name": "cantor",
            "version": env!("CARGO_PKG_VERSION"),
            "methods": METHODS,
        })),
        "open_project" => to_value(engine.open_project(&params::<PathP>(p)?.path)?),
        "new_project" => to_value(engine.new_project(&params::<NewP>(p)?.title)),
        "import_file" => to_value(engine.import_file(&params::<PathP>(p)?.path)?),
        "save_project" => {
            let a: SaveP = params(p)?;
            to_value(engine.save_project(&a.project_id, a.path.as_deref())?)
        }
        "close_project" => to_value(engine.close_project(&params::<ProjectP>(p)?.project_id)?),
        "get_project" => {
            let (project, revision) = engine.snapshot(&params::<ProjectP>(p)?.project_id)?;
            Ok(json!({ "revision": revision, "project": *project }))
        }
        "describe_project" => {
            to_value(engine.describe_project(&params::<ProjectP>(p)?.project_id)?)
        }
        "describe_range" => {
            let a: RangeP = params(p)?;
            to_value(engine.describe_range(
                &a.project_id,
                a.track.as_ref(),
                TickRange {
                    start: a.t0,
                    end: a.t1,
                },
                a.detail,
            )?)
        }
        "list_singers" => to_value(engine.list_singers()),
        "list_phonemizers" => to_value(engine.list_phonemizers()),
        "get_singer_schema" => to_value(engine.get_singer_schema(&params::<SingerP>(p)?.singer)?),
        "apply_ops" => {
            let a: OpsP = params(p)?;
            to_value(engine.apply_ops(&a.project_id, a.base_revision, a.ops, a.dry_run)?)
        }
        "retake" => {
            let a: RetakeP = params(p)?;
            to_value(engine.retake(
                &a.project_id,
                a.base_revision,
                &a.track,
                a.range,
                a.dims,
                a.n_variants,
                a.seed,
            )?)
        }
        "select_take" => {
            let a: SelectP = params(p)?;
            to_value(engine.select_take(
                &a.project_id,
                a.base_revision,
                PhraseId(a.phrase_id),
                a.take_id.map(TakeId),
            )?)
        }
        "render" => {
            let a: RenderP = params(p)?;
            to_value(engine.render(&a.project_id, a.range, a.quality)?)
        }
        "compare" => {
            let a: CompareP = params(p)?;
            to_value(engine.compare(&a.render_a, &a.render_b)?)
        }
        "checkpoint" => {
            let a: NameP = params(p)?;
            to_value(engine.checkpoint(&a.project_id, &a.name)?)
        }
        "restore" => {
            let a: RestoreP = params(p)?;
            to_value(engine.restore(&a.project_id, a.base_revision, &a.name)?)
        }
        "list_history" => to_value(engine.list_history(&params::<ProjectP>(p)?.project_id)?),
        "export" => {
            let a: ExportP = params(p)?;
            to_value(engine.export(&a.project_id, &a.format, &a.path)?)
        }
        other => Err(RpcError::new(-32601, format!("method not found: {other}"))),
    }
}

/// Handles one JSON-RPC message (request, notification or batch). Returns
/// the response text, or `None` when nothing should be sent back.
pub fn handle(engine: &Engine, text: &str) -> Option<String> {
    let msg: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            return Some(error_response(
                Value::Null,
                &RpcError::new(-32700, format!("parse error: {e}")),
            ));
        }
    };
    match msg {
        Value::Array(items) if items.is_empty() => Some(error_response(
            Value::Null,
            &RpcError::new(-32600, "empty batch"),
        )),
        Value::Array(items) => {
            let out: Vec<Value> = items
                .into_iter()
                .filter_map(|m| handle_one(engine, m))
                .collect();
            (!out.is_empty()).then(|| Value::Array(out).to_string())
        }
        m => handle_one(engine, m).map(|v| v.to_string()),
    }
}

fn handle_one(engine: &Engine, msg: Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let valid = msg.get("jsonrpc") == Some(&json!("2.0"))
        && msg.get("method").is_some_and(Value::is_string);
    if !valid {
        return Some(
            serde_json::from_str(&error_response(
                id.unwrap_or(Value::Null),
                &RpcError::new(-32600, "invalid request"),
            ))
            .expect("valid json"),
        );
    }
    let method = msg["method"].as_str().expect("checked").to_owned();
    let p = msg.get("params").cloned().unwrap_or(Value::Null);
    let result = dispatch(engine, &method, p);
    let id = id?; // notifications get no response
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": e.to_json() }),
    })
}

fn error_response(id: Value, e: &RpcError) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": e.to_json() }).to_string()
}

/// Serves JSON-RPC on stdin/stdout, one message per line, until EOF.
pub fn serve_stdio(engine: &Engine) -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(resp) = handle(engine, &line) {
            writeln!(stdout, "{resp}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// Serves JSON-RPC over WebSocket at `/rpc` on `addr` (one message per
/// text frame). Binds loopback addresses only.
pub async fn serve_ws(engine: Arc<Engine>, addr: std::net::SocketAddr) -> std::io::Result<()> {
    if !addr.ip().is_loopback() {
        return Err(std::io::Error::other(
            "the JSON-RPC server binds loopback addresses only",
        ));
    }
    let listener = tokio::net::TcpListener::bind(addr).await?;
    serve_ws_listener(engine, listener).await
}

/// Like [`serve_ws`], on an existing listener.
pub async fn serve_ws_listener(
    engine: Arc<Engine>,
    listener: tokio::net::TcpListener,
) -> std::io::Result<()> {
    use axum::extract::State;
    use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
    use axum::routing::get;

    async fn session(mut socket: WebSocket, engine: Arc<Engine>) {
        while let Some(Ok(msg)) = socket.recv().await {
            let Message::Text(text) = msg else {
                continue;
            };
            let e = engine.clone();
            let text = text.to_string();
            let resp = tokio::task::spawn_blocking(move || handle(&e, &text))
                .await
                .ok()
                .flatten();
            if let Some(resp) = resp
                && socket.send(Message::Text(resp.into())).await.is_err()
            {
                break;
            }
        }
    }

    let app = axum::Router::new()
        .route(
            "/rpc",
            get(
                |ws: WebSocketUpgrade, State(engine): State<Arc<Engine>>| async move {
                    ws.on_upgrade(move |socket| session(socket, engine))
                },
            ),
        )
        .with_state(engine);
    axum::serve(listener, app).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineConfig;

    fn engine() -> Engine {
        let tmp = std::env::temp_dir().join(format!("cantor-rpc-{}", std::process::id()));
        Engine::new(EngineConfig {
            singer_paths: vec![],
            vocoder_paths: vec![],
            phonemizer_paths: vec![],
            rmvpe: None,
            cache_dir: tmp.join("cache"),
            render_dir: tmp.join("renders"),
        })
    }

    fn call(e: &Engine, req: Value) -> Value {
        serde_json::from_str(&handle(e, &req.to_string()).unwrap()).unwrap()
    }

    #[test]
    fn requests_errors_batches_and_notifications() {
        let e = engine();
        let r = call(
            &e,
            json!({"jsonrpc": "2.0", "id": 1, "method": "new_project", "params": {"title": "T"}}),
        );
        let pid = r["result"]["project_id"].as_str().unwrap().to_owned();
        let ops = json!([{ "op": "set_tempo", "tick": 0, "bpm": 96.0 }]);
        let ok = call(
            &e,
            json!({"jsonrpc": "2.0", "id": 2, "method": "apply_ops",
            "params": {"project_id": pid, "base_revision": 0, "ops": ops}}),
        );
        assert_eq!(ok["result"]["applied"], json!(true));
        let stale = call(
            &e,
            json!({"jsonrpc": "2.0", "id": 3, "method": "apply_ops",
            "params": {"project_id": pid, "base_revision": 0, "ops": ops}}),
        );
        assert_eq!(stale["error"]["code"], json!(-32002));
        assert_eq!(stale["error"]["data"]["current_revision"], json!(1));
        assert_eq!(
            stale["error"]["data"]["diff"]["modified"][0]["kind"],
            json!("tempo_map")
        );

        let missing = call(&e, json!({"jsonrpc": "2.0", "id": 4, "method": "nope"}));
        assert_eq!(missing["error"]["code"], json!(-32601));
        let bad = call(
            &e,
            json!({"jsonrpc": "2.0", "id": 5, "method": "apply_ops", "params": {"x": 1}}),
        );
        assert_eq!(bad["error"]["code"], json!(-32602));
        assert_eq!(
            serde_json::from_str::<Value>(&handle(&e, "{oops").unwrap()).unwrap()["error"]["code"],
            json!(-32700)
        );

        // A notification gets no response; a batch gets one per request.
        assert!(
            handle(
                &e,
                &json!({"jsonrpc": "2.0", "method": "list_singers"}).to_string()
            )
            .is_none()
        );
        let batch = call(
            &e,
            json!([
                {"jsonrpc": "2.0", "id": 6, "method": "server/discover"},
                {"jsonrpc": "2.0", "method": "list_singers"},
                {"jsonrpc": "2.0", "id": 7, "method": "describe_project", "params": {"project_id": pid}}
            ]),
        );
        assert_eq!(batch.as_array().unwrap().len(), 2);
        assert!(batch[1]["result"].as_str().unwrap().contains("96 BPM"));
    }
}
