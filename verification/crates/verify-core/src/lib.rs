//! Transport-free correctness verification infrastructure.
//!
//! Catalogs are parsed from caller-supplied text and validated against a
//! caller-supplied backend registry. The library reads no files or environment
//! variables and never invokes a backend during validation.

mod backend;
mod catalog;
mod conformance;
mod identity;
mod target;

pub use backend::*;
pub use catalog::*;
pub use conformance::*;
pub use identity::*;
pub use target::*;
