//! Surface-neutral error type for dispatch operations.
//!
//! `ToolError` is the single canonical error type across all three surfaces
//! (MCP, API, CLI). It now lives in `labby_runtime::error` so the
//! gateway-extraction crates can share it; this module re-exports it for the
//! existing `crate::dispatch::error::ToolError` import path.
//!
//! Service-specific `From<ServiceError> for ToolError` impls live beside the
//! remaining services that need them.

pub use labby_runtime::error::ToolError;
