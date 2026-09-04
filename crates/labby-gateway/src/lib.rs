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
//! The product crate re-exports gateway types from `crates/labby/src/dispatch/upstream.rs`
//! as a compatibility shim for existing Labby callers.

pub mod codemode_journal;
pub mod core_provider;
pub mod dispatch_helpers;
pub mod gateway;
pub mod net;
pub mod process;
pub mod registry;
pub mod security;
pub mod upstream;
pub mod usage;

pub use labby_primitives::mcp::{
    MCP_RELAY_CANCELLATION_REQUEST_METHOD, MCP_RELAY_CANCELLATION_TOKEN_META_KEY,
};

#[cfg(test)]
mod test_support;
