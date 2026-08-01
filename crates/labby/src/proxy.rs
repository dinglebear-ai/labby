//! Ephemeral stdio MCP proxy runtime.

pub mod command;
pub mod config;
#[cfg(feature = "gateway")]
pub mod runtime;
#[cfg(feature = "gateway")]
pub mod tailscale;

#[cfg(test)]
#[cfg(feature = "gateway")]
mod runtime_tests;
