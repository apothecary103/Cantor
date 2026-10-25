# 0013 MCP: rmcp, Streamable HTTP, official conformance suite

- **SDK:** the official Rust SDK `rmcp` pinned at `=3.7.0` (features
  server, macros, elicitation, request-state,
  transport-streamable-http-server), served with `axum =0.8.9` (http1 only)
  on `tokio =1.54.1`. rmcp 3.7 implements spec revision 2026-07-28,
  including the `Mcp-Method` request header (requests without it get
  -32020) and input-required results with request state.
- **Transport:** `cantor_mcp::http` serves any `ServerHandler` over
  Streamable HTTP on loopback only, and rejects a foreign `Host` or
  `Origin` with 403 (DNS-rebinding protection). The handler factory runs
  per request, so it must be cheap (clone an `Arc` of shared state).
- **Conformance gate:** `cargo xtask conformance` builds the
  `cantor-mcp-conformance` fixture server (rmcp's reference conformance
  server at tag rmcp-v3.7.0, kept whole with its license header), starts it
  on 127.0.0.1:8001, and runs
  `npx @modelcontextprotocol/conformance@0.2.0-alpha.12 server
  --requirements 2026-07-28`. The 0.1.x line does not know 2026-07-28, so
  the alpha is pinned exactly. Only required scenarios decide the exit
  code; extension (tasks) and pending scenarios are reported but not
  scored. An optional baseline at
  `crates/cantor-mcp/conformance/expected-failures.yaml` is passed as
  `--expected-failures` when present; none is needed today.
- **Result (2026-10-25):** 197 checks passed, 0 failed, across all required
  scenarios; the 13 unscored scenarios also pass.
- **Runs in its own CI job,** not in `cargo xtask ci`, because it needs
  the network for npx. Node comes from the flake (`nodejs_22`). xtask sets
  `npm_config_cache` to `<CARGO_TARGET_DIR or target>/npm-cache` unless
  it is already set, so runs never fill `~/.npm`.
- **Next:** the real Cantor server (tools over `cantor-core` Ops) reuses
  `cantor_mcp::http`; its own scenarios join the suite as it grows.
