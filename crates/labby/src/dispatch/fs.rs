//! Jailed read-only workspace filesystem service.
//!
//! The service resolves the configured `[workspace].root`, exposes bounded
//! `fs.list` discovery through MCP/API/web, and serves bounded `fs.preview`
//! content through HTTP/web. Requested paths stay beneath the configured root;
//! missing configuration returns the structured `workspace_not_configured`
//! error.
//!
//! This is a Labby-local product service with no `labby-apis` counterpart. It
//! is compiled by the `fs` feature in `crates/labby/Cargo.toml` and intentionally
//! exposes no general-purpose write/delete/rename surface.

pub mod catalog;
pub mod client;

#[cfg(feature = "fs")]
pub mod dispatch;
#[cfg(feature = "fs")]
pub mod params;

#[cfg(feature = "fs")]
pub(crate) use client::not_configured_error;

#[cfg(feature = "fs")]
pub use dispatch::{dispatch, dispatch_with_root, open_for_preview};
