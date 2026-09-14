//! Bounded concurrency adapters for project-owned Loom and Shuttle harnesses.
//!
//! These adapters verify only small harnesses built from each engine's
//! instrumented primitives. They do not claim coverage of Tokio or the
//! production async runtime.

mod adapter;

pub use adapter::*;
