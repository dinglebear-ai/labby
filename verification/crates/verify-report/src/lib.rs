//! Report contract and renderers for verification results.
//!
//! The JSON form is the stable contract; text, Markdown, and HTML all render
//! from it rather than being generated independently, so two surfaces cannot
//! disagree about what happened.
//!
//! This crate starts minimal — the replay report plus a text renderer — so that
//! adding coverage matrices at M5 does not mean moving a public type out of the
//! runner and breaking every caller.

pub mod replay;
pub mod text;

pub use replay::{Outcome, ReplayReport, StepRecord, Summary};
pub use text::render_text;
