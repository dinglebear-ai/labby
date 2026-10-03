//! Code Mode snippet ENGINE (store, parse, validate, resolve, render).
//!
//! This is the storage/resolution engine only. The snippet SURFACE (the MCP
//! tool, HTTP route, CLI command, and `ACTIONS` catalog) lives in the host
//! binary as a thin adapter over this module.

/// Deterministic fixture execution using the production parser and sandbox.
pub mod harness;

/// Offline schema contracts and synthetic fixture value generation.
pub mod schemas;

/// Snippet storage, validation, resolution, and input-merging primitives.
pub mod store;
/// Validated, presence-aware exact-tool declarations for saved snippets.
pub mod tool_declarations;
