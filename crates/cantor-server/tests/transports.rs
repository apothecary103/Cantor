//! JSON-RPC over the stdio binary and over WebSocket.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::Arc;

#[test]
fn stdio_binary_answers_line_by_line() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("rpc-stdio");
    let mut child = Command::new(env!("CARGO_BIN_EXE_cantor-server"))
        .env("CANTOR_CACHE", &dir)
        .env_remove("CANTOR_SINGERS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut ask = |req: Value| -> Value {
        writeln!(stdin, "{req}").unwrap();
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    };
    let d = ask(json!({"jsonrpc": "2.0", "id": 1, "method": "server/discover"}));
    assert!(
        d["result"]["methods"]
            .as_array()
            .unwrap()
            .contains(&json!("render"))
    );
    let p =
        ask(json!({"jsonrpc": "2.0", "id": 2, "method": "new_project", "params": {"title": "x"}}));
    assert_eq!(p["result"]["revision"], json!(0));
    drop(ask);
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

/// A minimal WebSocket client: handshake, one masked text frame, one reply.
async fn ws_roundtrip(addr: std::net::SocketAddr, text: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "GET /rpc HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        head.push(s.read_u8().await.unwrap());
    }
    assert!(String::from_utf8_lossy(&head).starts_with("HTTP/1.1 101"));
    let payload = text.as_bytes();
    let mask = [1u8, 2, 3, 4];
    let mut frame = vec![0x81];
    if payload.len() < 126 {
        frame.push(0x80 | payload.len() as u8);
    } else {
        frame.push(0x80 | 126);
        frame.extend((payload.len() as u16).to_be_bytes());
    }
    frame.extend(mask);
    frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    s.write_all(&frame).await.unwrap();
    let b0 = s.read_u8().await.unwrap();
    assert_eq!(b0 & 0x0f, 1, "text frame");
    let len = match s.read_u8().await.unwrap() & 0x7f {
        126 => usize::from(s.read_u16().await.unwrap()),
        127 => s.read_u64().await.unwrap() as usize,
        n => usize::from(n),
    };
    let mut buf = vec![0; len];
    s.read_exact(&mut buf).await.unwrap();
    String::from_utf8(buf).unwrap()
}

#[tokio::test]
async fn websocket_serves_json_rpc() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("rpc-ws");
    let engine = Arc::new(cantor_server::Engine::new(cantor_server::EngineConfig {
        singer_paths: vec![],
        vocoder_paths: vec![],
        phonemizer_paths: vec![],
        rmvpe: None,
        cache_dir: dir.join("cache"),
        render_dir: dir.join("renders"),
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(cantor_server::rpc::serve_ws_listener(engine, listener));
    let reply = ws_roundtrip(
        addr,
        r#"{"jsonrpc":"2.0","id":7,"method":"new_project","params":{"title":"ws"}}"#,
    )
    .await;
    let v: Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(v["id"], json!(7));
    assert_eq!(v["result"]["title"], json!("ws"));
}
