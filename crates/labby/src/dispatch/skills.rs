//! Internal dispatch for the native Agent Skills protocol.
//!
//! These operations back `skills/list`, `skills/get`, and `resources/read`.
//! They are not registered as Labby action-service aliases; managed lifecycle
//! operations live exclusively under `artifacts.*`.

pub mod catalog;
pub mod client;
pub mod dispatch;
pub mod params;
pub mod types;

pub(crate) use catalog::api_actions;
pub(crate) use dispatch::dispatch_with_context;

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
