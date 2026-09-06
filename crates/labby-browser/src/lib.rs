//! Surface-neutral Rust runtime for Labby's browser-native WebMCP bridge.
//!
//! The browser extension speaks a small versioned JSON protocol. This crate
//! owns its durable SQLite state, pairing and authentication, live connection
//! registry, bounded invocation routing, cancellation, and metadata-only
//! audits. HTTP, MCP, CLI, and web handlers are adapters around this runtime.

mod error;
mod hub;
mod protocol;
mod store;

pub use error::{BrowserError, Result};
pub use hub::{BrowserBridge, BrowserConnection, BrowserEvent};
pub use protocol::{BrowserEnvelope, BrowserMessage, CatalogObservation, ToolDescriptor};
pub use store::{
    BrowserRecord, DocumentSession, DocumentSessionSummary, PairingRequest, PairingStatus,
    SessionPage, Store,
};
