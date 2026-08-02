#![forbid(unsafe_code)]

//! Standalone gateway runtime.
//!
//! This crate owns the upstream MCP proxy pool: connection management to
//! external MCP servers (HTTP, websocket, or stdio), tool/resource/prompt
//! discovery, circuit breaking, subject-scoped OAuth connections, relay
//! sessions, and in-process service-peer registration.
//!
//! It is surface-neutral: it does not depend on `axum`, `clap`, `utoipa`, or
//! Labby's default service-registry builder. Callers inject the small seams it
//! needs (an in-process connector and a service registry) through the traits in
//! [`registry`].
//!
//! The pool is re-exported from `lab`'s `crate::dispatch::upstream` as a
//! compatibility shim so existing Labby callers keep working unchanged.

pub mod codemode_journal;
pub mod dispatch_helpers;
pub mod gateway;
pub mod net;
pub mod process;
pub mod registry;
pub mod security;
pub mod upstream;
pub mod usage;

/// Private MCP `_meta` key correlating stateless HTTP cancellation posts with
/// the original relayed request. The value is a random per-request token.
pub const MCP_RELAY_CANCELLATION_TOKEN_META_KEY: &str =
    "ai.dinglebear.labby/relayCancellationToken";

/// Labby-private request used alongside standard cancellation when an rmcp
/// stateless HTTP hop hides or rewrites request IDs.
pub const MCP_RELAY_CANCELLATION_REQUEST_METHOD: &str = "ai.dinglebear.labby/relay-cancel";

#[cfg(test)]
mod test_support;
