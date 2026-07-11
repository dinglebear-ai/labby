//! MCP-specific service exception modules.
//!
//! Normal services register directly from `crate::dispatch::<service>` in
//! `crate::registry`. This module only declares adapters that own behavior
//! specific to the MCP surface and cannot be represented by shared dispatch
//! alone.

#[cfg(feature = "fs")]
pub mod fs;
#[cfg(feature = "gateway")]
pub mod snippets;
