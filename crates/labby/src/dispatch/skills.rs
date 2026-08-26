//! Shared compatibility dispatch service for Agent Skills.
//!
//! Native SEP methods remain protocol-specific; this service projects the same
//! canonical registry into Labby's action-oriented CLI/API/MCP surface.

pub mod catalog;
pub mod client;
pub mod dispatch;
pub mod params;
pub mod types;

pub(crate) use catalog::{ACTIONS, API_ACTIONS};
#[cfg(feature = "gateway")]
pub(crate) use dispatch::{dispatch, dispatch_with_context};

use labby_primitives::plugin::{Category, PluginMeta};

pub const META: PluginMeta = PluginMeta {
    name: "skills",
    display_name: "Skills",
    description: "Discover, read, and manage Agent Skills through one shared service",
    category: Category::Bootstrap,
    docs_url: "https://github.com/dinglebear-ai/labby",
    required_env: &[],
    optional_env: &[],
    default_port: None,
    supports_multi_instance: false,
};
