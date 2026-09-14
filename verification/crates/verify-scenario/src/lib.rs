//! Portable scenario data; no model semantics, backend execution, or implicit IO.

mod envelope;
mod fingerprint;
pub mod normalize;
mod strict_json;

pub use envelope::*;
pub use fingerprint::content_fingerprint;
pub use normalize::normalize_syntactic;
