# 0015 Engine, JSON-RPC and MCP surfaces

- **One engine** (`cantor-server::Engine`) owns open projects, history,
  renders and the singer registry. JSON-RPC methods, MCP tools and the
  Python API are thin wrappers, so no capability exists on one surface only.
- **Optimistic concurrency:** every mutating call takes `base_revision`;
  a mismatch fails with `conflict` carrying the current revision and the
  merged diff since the base (JSON-RPC −32002 with `data`, MCP `isError`
  structured result). Revisions increase on undo/redo/restore too.
- **MCP 2026-07-28 (stateless):** rmcp 3.7 handles discover and per-request
  metadata; state lives in server-minted handles (`project_id`,
  `render_id`) passed as tool arguments. Tools return structured content;
  errors are `isError` results so agents can react. Annotations: read-only
  for describe/list/get/compare/list_history; destructive for apply_ops,
  retake, select_take, restore, export; render is side-effecting but not
  destructive. Resources: schemas, parameter registry, singer schemas,
  project JSON and render reports by URI.
- **Conformance:** the official suite calls fixed fixture tools, so it runs
  against a separate fixture server built on `cantor-mcp::http` (0013).
  Cantor's own server is covered by the agent-loop test (MCP-only tuning to
  < 20 cents p95) and annotation/resource tests.
- **Transports:** MCP over stdio or Streamable HTTP (loopback, Host/Origin
  checked); JSON-RPC over stdio (one message per line) or WebSocket `/rpc`
  (loopback only). Batches and notifications follow JSON-RPC 2.0.
- **Engine configuration** comes from `CANTOR_SINGERS`, `CANTOR_VOCODERS`
  (path lists), `CANTOR_RMVPE`, `CANTOR_CACHE`, `CANTOR_RENDERS`. Vocoders
  are matched by directory name or the `name:` in `vocoder.yaml`.
- One singer per render call for now; multi-singer projects return a clear
  error.
