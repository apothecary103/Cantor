//! Cantor's MCP server for AI agents (docs/spec/60-api.md).
//!
//! [`server::CantorMcp`] exposes the engine as tools, resources and prompts.
//! [`http`] serves any rmcp [`rmcp::ServerHandler`] over Streamable HTTP on
//! loopback. The `cantor-mcp-conformance` binary uses it to run the official
//! MCP conformance suite (`cargo xtask conformance`, decision 0013).

#![forbid(unsafe_code)]

pub mod http;
pub mod server;

pub use server::CantorMcp;
