//! Streamable HTTP transport: an rmcp [`ServerHandler`] mounted at
//! [`MCP_PATH`] on an axum router, reachable from loopback only.
//!
//! Requests negotiating MCP 2026-07-28 are always served statelessly; older
//! protocol versions get rmcp's in-memory sessions. Every request is checked
//! against DNS rebinding: the `Host` header must name a loopback host and a
//! present `Origin` must be a loopback `http` origin, otherwise rmcp answers
//! `403 Forbidden`.

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use rmcp::ServerHandler;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::net::TcpListener;

/// The path the MCP endpoint is served at.
pub const MCP_PATH: &str = "/mcp";

/// `Host` values accepted on any port.
pub const LOOPBACK_HOSTS: &[&str] = &["localhost", "127.0.0.1", "::1"];

/// `Origin` values accepted (browsers on the same machine, any port).
pub const LOOPBACK_ORIGINS: &[&str] =
    &["http://localhost:*", "http://127.0.0.1:*", "http://[::1]:*"];

/// rmcp's Streamable HTTP configuration restricted to loopback `Host` and
/// `Origin` values, with Origin validation enforced.
pub fn loopback_config() -> StreamableHttpServerConfig {
    StreamableHttpServerConfig::default()
        .with_allowed_hosts(LOOPBACK_HOSTS.iter().copied())
        .with_allowed_origins(LOOPBACK_ORIGINS.iter().copied())
        .enforce_origin_validation()
}

/// An axum router serving `factory`'s handlers at [`MCP_PATH`].
///
/// `factory` runs once per stateless request (and once per legacy session),
/// so it should be cheap: clone an `Arc` of shared state.
pub fn router<S, F>(factory: F, config: StreamableHttpServerConfig) -> axum::Router
where
    S: ServerHandler + Send + 'static,
    F: Fn() -> S + Send + Sync + 'static,
{
    let service = StreamableHttpService::new(
        move || Ok(factory()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    axum::Router::new().nest_service(MCP_PATH, service)
}

/// Serves `factory`'s handlers on `listener` with [`loopback_config`] until
/// `shutdown` resolves.
pub async fn serve_listener<S, F>(
    listener: TcpListener,
    factory: F,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> io::Result<()>
where
    S: ServerHandler + Send + 'static,
    F: Fn() -> S + Send + Sync + 'static,
{
    let config = loopback_config();
    let cancel = config.cancellation_token.clone();
    let app = router(factory, config);
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown.await;
            // Ends open SSE streams so graceful shutdown can finish.
            cancel.cancel();
        })
        .await
}

/// Binds `addr` and serves `factory`'s handlers at `http://{addr}/mcp` with
/// [`loopback_config`] until the process gets Ctrl-C.
///
/// `addr` should be a loopback address; the `Host` check rejects requests
/// that name any other host anyway.
pub async fn serve_streamable_http<S, F>(addr: SocketAddr, factory: F) -> io::Result<()>
where
    S: ServerHandler + Send + 'static,
    F: Fn() -> S + Send + Sync + 'static,
{
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(
        "MCP server listening on http://{}{MCP_PATH}",
        listener.local_addr()?
    );
    serve_listener(listener, factory, async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::{Implementation, ServerCapabilities, ServerConfig};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[derive(Clone)]
    struct Empty;

    impl ServerHandler for Empty {
        fn get_info(&self) -> ServerConfig {
            ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
                .with_server_info(Implementation::new("cantor-mcp-test", "0.0.0"))
        }
    }

    async fn start() -> (SocketAddr, tokio::sync::oneshot::Sender<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(serve_listener(listener, || Empty, async {
            let _ = rx.await;
        }));
        (addr, tx)
    }

    /// Sends one HTTP/1.1 POST and returns the status line and body.
    async fn post(addr: SocketAddr, host: &str, origin: Option<&str>, body: &str) -> String {
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let origin = origin
            .map(|o| format!("Origin: {o}\r\n"))
            .unwrap_or_default();
        let request = format!(
            "POST {MCP_PATH} HTTP/1.1\r\nHost: {host}\r\n{origin}\
             Content-Type: application/json\r\n\
             Accept: application/json, text/event-stream\r\n\
             MCP-Protocol-Version: 2026-07-28\r\n\
             Mcp-Method: server/discover\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        response
    }

    const DISCOVER: &str = r#"{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{},"io.modelcontextprotocol/clientInfo":{"name":"test","version":"0"}}}}"#;

    #[tokio::test]
    async fn serves_discover_on_loopback() {
        let (addr, _stop) = start().await;
        let response = post(addr, &addr.to_string(), None, DISCOVER).await;
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.contains("cantor-mcp-test"), "{response}");
    }

    #[tokio::test]
    async fn rejects_foreign_host() {
        let (addr, _stop) = start().await;
        let response = post(addr, "evil.example", None, DISCOVER).await;
        assert!(response.starts_with("HTTP/1.1 403"), "{response}");
    }

    #[tokio::test]
    async fn rejects_foreign_origin() {
        let (addr, _stop) = start().await;
        let host = addr.to_string();
        let response = post(addr, &host, Some("http://evil.example"), DISCOVER).await;
        assert!(response.starts_with("HTTP/1.1 403"), "{response}");
        let response = post(addr, &host, Some(&format!("http://{host}")), DISCOVER).await;
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    }
}
