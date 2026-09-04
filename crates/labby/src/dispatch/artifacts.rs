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
        super::skill_library::catalog::LOCAL_ACTIONS
            .into_iter()
            .chain(
                super::remote_control::REMOTE_ARTIFACT_ACTIONS
                    .iter()
                    .copied(),
            )
            .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_catalog_is_the_exact_canonical_composition() {
        let names = ACTIONS.iter().map(|action| action.name).collect::<Vec<_>>();
        let unique = names
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(names.len(), unique.len());
        assert_eq!(
            names.len(),
            crate::dispatch::skill_library::catalog::LOCAL_ACTIONS.len()
                + crate::dispatch::remote_control::REMOTE_ARTIFACT_ACTIONS.len()
        );
        let rendered = ACTIONS
            .iter()
            .map(|action| format!("{action:?}"))
            .collect::<Vec<_>>();
        let expected = crate::dispatch::skill_library::catalog::LOCAL_ACTIONS
            .iter()
            .chain(crate::dispatch::remote_control::REMOTE_ARTIFACT_ACTIONS)
            .map(|action| format!("{action:?}"))
            .collect::<Vec<_>>();
        assert_eq!(rendered, expected);
    }
}
