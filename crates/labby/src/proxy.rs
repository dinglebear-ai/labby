//! Ephemeral stdio MCP proxy runtime.

pub mod command;
pub mod config;
#[cfg(feature = "gateway")]
pub mod runtime;

#[cfg(test)]
#[cfg(feature = "gateway")]
mod runtime_tests;
