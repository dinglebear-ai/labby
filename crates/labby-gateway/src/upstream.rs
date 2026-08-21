//! Upstream MCP server proxy — shared types and connection pool.
//!
//! This is the runtime home of `UpstreamPool`. It is re-exported from Labby's
//! `crate::dispatch::upstream` compatibility shim so every existing surface
//! (CLI, MCP, HTTP API) keeps the same import path. The pool is surface-neutral:
//! both the MCP and API surfaces need access to it, and the layer contract
//! forbids `api -> mcp` dependencies, so it cannot live under either surface.
//
// Some public gateway primitives are exercised only by specific surfaces or tests.
// Keep the scoped dead-code allows on those modules rather than inventing product
// dependencies solely to satisfy the compiler.
#[allow(dead_code)]
pub mod auth;
pub mod direct_stdio;
#[allow(dead_code)]
pub mod http_client;
#[allow(dead_code)]
pub mod pool;
#[allow(dead_code)]
pub mod process_guard;
pub use crate::security::spawn_guard;
pub mod tool_error;
#[allow(dead_code)]
pub mod transport;
#[allow(dead_code)]
pub mod types;
