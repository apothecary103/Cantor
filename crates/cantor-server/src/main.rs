//! `cantor-server`: JSON-RPC 2.0 over stdio (default) or WebSocket
//! (`--ws 127.0.0.1:8766`, endpoint `/rpc`).

use cantor_server::{Engine, EngineConfig, rpc};
use std::process::ExitCode;
use std::sync::Arc;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let engine = Arc::new(Engine::new(EngineConfig::from_env()));
    let result = match args.as_slice() {
        [] => rpc::serve_stdio(&engine).map_err(|e| e.to_string()),
        [flag, addr] if flag == "--ws" => match addr.parse() {
            Ok(addr) => tokio::runtime::Runtime::new()
                .map_err(|e| e.to_string())
                .and_then(|rt| {
                    rt.block_on(rpc::serve_ws(engine, addr))
                        .map_err(|e| e.to_string())
                }),
            Err(e) => Err(format!("bad address {addr}: {e}")),
        },
        _ => Err("usage: cantor-server [--ws ADDR]".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cantor-server: {e}");
            ExitCode::FAILURE
        }
    }
}
