//! Artifact control-plane service metadata.
//!
//! Authenticated API and MCP adapters terminate authorization before invoking
//! the managed Artifact core. The generic registry dispatcher therefore fails
//! closed instead of manufacturing an identity or project context.

use labby_primitives::plugin::{Category, PluginMeta};
use serde_json::Value;
use std::sync::LazyLock;

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{action_schema, help_payload, require_str};

pub(crate) static ACTIONS: LazyLock<Vec<labby_primitives::action::ActionSpec>> =
    LazyLock::new(|| {
        let mut actions = super::skill_library::catalog::ACTIONS.to_vec();
        for action in super::remote_control::REMOTE_ARTIFACT_ACTIONS {
            if !actions.iter().any(|existing| existing.name == action.name) {
                actions.push(*action);
            }
        }
        actions
    });

pub const META: PluginMeta = PluginMeta {
    name: "artifacts",
    display_name: "Artifacts",
    description: "Search, import, revise, validate, and activate durable Artifacts",
    category: Category::Bootstrap,
    docs_url: "https://github.com/dinglebear-ai/labby",
    required_env: &[],
    optional_env: &[],
    default_port: None,
    supports_multi_instance: false,
};

pub async fn dispatch(action: &str, params: Value) -> Result<Value, ToolError> {
    match action {
        "help" => return Ok(help_payload(META.name, &ACTIONS)),
        "schema" => return action_schema(&ACTIONS, require_str(&params, "action")?),
        _ => {}
    }
    if crate::dispatch::remote_control::REMOTE_ARTIFACT_ACTIONS
        .iter()
        .any(|candidate| candidate.name == action)
    {
        return crate::dispatch::remote_control::dispatch("artifacts", action, params).await;
    }
    Err(ToolError::Forbidden {
        message: "Artifact management requires an authenticated project-bound API or MCP request"
            .to_owned(),
        required_scopes: vec!["lab:read".to_owned()],
    })
}
