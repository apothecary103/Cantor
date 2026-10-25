//! The Cantor engine API and its JSON-RPC 2.0 server. Every client — the
//! GUI's remote mode, Python, the CLI and MCP — calls these same methods.

#![forbid(unsafe_code)]

pub mod convert;
pub mod describe;
pub mod engine;
pub mod rpc;
pub mod singers;

pub use engine::{Engine, EngineConfig, EngineError, TrackRef};
