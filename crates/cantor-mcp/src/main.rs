//! `cantor-mcp`: Cantor's MCP server over stdio (default) or Streamable
//! HTTP (`--http 127.0.0.1:8765`). Singers, vocoders and the RMVPE model
//! come from CANTOR_SINGERS, CANTOR_VOCODERS and CANTOR_RMVPE.

use cantor_mcp::CantorMcp;
use cantor_server::{Engine, EngineConfig};
use rmcp::ServiceExt;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_env("CANTOR_LOG"))
        .with_writer(std::io::stderr)
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let engine = Arc::new(Engine::new(EngineConfig::from_env()));
    let result = match args.as_slice() {
        [flag, addr] if flag == "--http" => match addr.parse::<SocketAddr>() {
            Ok(addr) => cantor_mcp::http::serve_streamable_http(addr, move || {
                CantorMcp::new(engine.clone())
            })
            .await
            .map_err(|e| e.to_string()),
            Err(e) => Err(format!("bad address {addr}: {e}")),
        },
        [] => match CantorMcp::new(engine).serve(rmcp::transport::stdio()).await {
            Ok(running) => running
                .waiting()
                .await
                .map(|_| ())
                .map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        },
        _ => Err("usage: cantor-mcp [--http ADDR]".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cantor-mcp: {e}");
            ExitCode::FAILURE
        }
    }
}
