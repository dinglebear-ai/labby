//! Stable, transport-free verification reports with evidence-separated lanes.

mod model;
mod render;

pub use model::*;
pub use render::{render_html, render_json, render_markdown, render_text};
