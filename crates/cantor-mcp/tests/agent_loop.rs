//! M3 done-when: an agent loop that uses only MCP tools lowers the p95
//! pitch error of a fixture phrase below 20 cents.
//!
//! The "agent" is a scripted client: it talks to Cantor exclusively through
//! MCP tool calls over an in-process transport (protocol 2026-07-28,
//! stateless discover lifecycle), exactly as an LLM agent would.

use cantor_mcp::CantorMcp;
use cantor_server::{Engine, EngineConfig};
use rmcp::model::{CallToolRequestParams, ProtocolVersion};
use rmcp::service::RunningService;
use rmcp::{ClientHandler, ClientLifecycleMode, ClientServiceExt, RoleClient, ServiceExt};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

const TWINKLE: &str = include_str!("../../cantor-cli/tests/fixtures/twinkle.cantor");

#[derive(Clone)]
struct Agent;
impl ClientHandler for Agent {}

fn engine(name: &str) -> Arc<Engine> {
    let vb = cantor_fixtures::ensure(&cantor_fixtures::QIXUAN).unwrap();
    cantor_fixtures::ensure(&cantor_fixtures::PC_NSF_HIFIGAN).unwrap();
    let rmvpe = cantor_fixtures::ensure(&cantor_fixtures::RMVPE).unwrap();
    let work = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&work);
    Arc::new(Engine::new(EngineConfig {
        singer_paths: vec![vb.parent().unwrap().to_path_buf()],
        vocoder_paths: vec![cantor_fixtures::dir()],
        rmvpe: Some(rmvpe),
        cache_dir: work.join("cache"),
        render_dir: work.join("renders"),
    }))
}

async fn connect(engine: Arc<Engine>) -> RunningService<RoleClient, Agent> {
    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move {
        let running = CantorMcp::new(engine)
            .serve(server_io)
            .await
            .expect("server starts");
        let _ = running.waiting().await;
    });
    Agent
        .serve_with_lifecycle(
            client_io,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .expect("client connects")
}

/// Calls a tool and returns its structured result, failing on tool errors.
async fn call(client: &RunningService<RoleClient, Agent>, tool: &str, args: Value) -> Value {
    let r = client
        .call_tool(
            CallToolRequestParams::new(tool.to_owned())
                .with_arguments(args.as_object().cloned().expect("object args")),
        )
        .await
        .unwrap_or_else(|e| panic!("{tool}: {e}"));
    assert_ne!(
        r.is_error,
        Some(true),
        "{tool} failed: {:?}",
        r.structured_content
    );
    r.structured_content
        .unwrap_or_else(|| panic!("{tool}: no structured content"))
}

#[tokio::test(flavor = "multi_thread")]
async fn tools_are_listed_with_accurate_annotations() {
    let client = connect(engine("annotations")).await;
    let tools = client.list_all_tools().await.unwrap();
    let ann = |name: &str| {
        tools
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("tool {name} missing"))
            .annotations
            .clone()
            .unwrap()
    };
    for name in [
        "describe_project",
        "describe_range",
        "list_singers",
        "get_singer_schema",
        "compare",
        "list_history",
    ] {
        assert_eq!(ann(name).read_only_hint, Some(true), "{name}");
    }
    for name in ["apply_ops", "restore", "retake"] {
        assert_eq!(ann(name).destructive_hint, Some(true), "{name}");
        assert_eq!(ann(name).read_only_hint, Some(false), "{name}");
    }
    let prompts = client.list_all_prompts().await.unwrap();
    let names: Vec<_> = prompts.iter().map(|p| p.name.as_str()).collect();
    assert!(
        names.contains(&"tune_phrase")
            && names.contains(&"breathier_chorus")
            && names.contains(&"add_harmony")
    );
    let resources = client.list_all_resources().await.unwrap();
    assert!(resources.iter().any(|r| r.uri == "cantor://schema/op"));
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_tunes_a_phrase_below_20_cents_with_mcp_only() {
    let client = connect(engine("agent")).await;
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("agent-project");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("twinkle.cantor");
    std::fs::write(&path, TWINKLE).unwrap();

    let opened = call(&client, "open_project", json!({ "path": path })).await;
    let pid = opened["project_id"].as_str().unwrap().to_owned();
    let mut revision = opened["revision"].as_u64().unwrap();
    let overview = call(&client, "describe_project", json!({ "project_id": pid })).await;
    let text = overview["text"].as_str().unwrap();
    assert!(text.contains("Phrase 100"), "{text}");

    // The fixture phrase: the first four notes (ticks 0..7680).
    let range = json!({ "start": 0, "end": 7680 });
    let mut corrections: BTreeMap<i64, f64> = BTreeMap::new();
    let mut p95s = Vec::new();
    let mut first_render = None;
    let mut last_render = String::new();
    for round in 0..6 {
        let report = call(
            &client,
            "render",
            json!({ "project_id": pid, "range": range, "quality": "preview" }),
        )
        .await;
        last_render = report["render_id"].as_str().unwrap().to_owned();
        first_render.get_or_insert_with(|| last_render.clone());
        let track = &report["analysis"]["tracks"][0];
        let p95 = track["pitch"]["p95_cents"].as_f64().unwrap();
        eprintln!("round {round}: phrase p95 {p95:.1} cents");
        p95s.push(p95);
        if p95 < 20.0 {
            break;
        }
        // Correct each note where the voice is off: subtract the measured
        // error from the pitch curve, tapering to 0 outside the note body.
        let mut ops = Vec::new();
        for note in track["notes"].as_array().unwrap() {
            let contour = note["error_contour"].as_array().unwrap();
            if contour.len() < 2 {
                continue;
            }
            for p in contour {
                *corrections
                    .entry(p["tick"].as_i64().unwrap())
                    .or_insert(0.0) -= p["cents"].as_f64().unwrap();
            }
            let first = contour[0]["tick"].as_i64().unwrap();
            let last = contour[contour.len() - 1]["tick"].as_i64().unwrap();
            let mut points = vec![json!([first - 80, 0.0])];
            points.extend(corrections.range(first..=last).map(|(t, v)| json!([t, v])));
            points.push(json!([last + 80, 0.0]));
            ops.push(json!({
                "op": "set_curve", "track": 1, "param": "pitch",
                "range": { "start": first - 80, "end": last + 81 },
                "points": points, "interp": "linear"
            }));
        }
        let applied = call(
            &client,
            "apply_ops",
            json!({ "project_id": pid, "base_revision": revision, "ops": ops }),
        )
        .await;
        assert_eq!(applied["applied"], json!(true), "{applied}");
        revision = applied["revision"].as_u64().unwrap();
    }
    let final_p95 = *p95s.last().unwrap();
    assert!(final_p95 < 20.0, "p95 history {p95s:?}");
    assert!(p95s[0] > final_p95);

    let cmp = call(
        &client,
        "compare",
        json!({ "render_a": first_render.unwrap(), "render_b": last_render }),
    )
    .await;
    assert!(cmp["metrics"]["pitch_p95_cents"]["delta"].as_f64().unwrap() < 0.0);

    // A stale base revision is reported as a conflict the agent can act on.
    let stale = client
        .call_tool(
            CallToolRequestParams::new("apply_ops").with_arguments(
                json!({ "project_id": pid, "base_revision": 0, "ops": [] })
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(stale.is_error, Some(true));
    let err = stale.structured_content.unwrap();
    assert_eq!(err["code"], "conflict");
    assert_eq!(err["current_revision"].as_u64(), Some(revision));
    client.cancel().await.unwrap();
}
