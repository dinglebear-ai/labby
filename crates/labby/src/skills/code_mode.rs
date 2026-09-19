//! Code Mode adapter over the canonical caller-authorized Skills registry.

use std::future::Future;
use std::sync::Arc;

use labby_codemode::{CodeModeCaller, ToolScope};
use labby_gateway::gateway::code_mode::{CodeModeSkillProvider, CodeModeSkillSummary};
use labby_runtime::error::ToolError;
use labby_runtime::skills::{FIRST_PARTY_ORIGIN, parse_skill_uri};
use serde_json::{Value, json};

use super::facade::{SkillRegistryContext, code_mode_skill_context};

#[derive(Debug, Default)]
pub(crate) struct CanonicalCodeModeSkillProvider;

fn context_for(caller: &CodeModeCaller) -> Result<Arc<SkillRegistryContext>, ToolError> {
    match caller {
        CodeModeCaller::ScopedSkills {
            skill_context_token,
            ..
        }
        | CodeModeCaller::ScopedHostProviderSkills {
            skill_context_token,
            ..
        } => code_mode_skill_context(skill_context_token).ok_or_else(|| ToolError::Sdk {
            sdk_kind: "forbidden".to_string(),
            message: "request-bound Skill context is unavailable or expired".to_string(),
        }),
        CodeModeCaller::TrustedLocal => Ok(Arc::new(SkillRegistryContext::first_party_only())),
        CodeModeCaller::Scoped { .. }
        | CodeModeCaller::ScopedPrivate { .. }
        | CodeModeCaller::ScopedHostProvider { .. } => Err(ToolError::Sdk {
            sdk_kind: "forbidden".to_string(),
            message: "request-bound Skill authorization context is unavailable".to_string(),
        }),
    }
}

fn origin_allowed(origin: &str, scope: &ToolScope) -> bool {
    origin == FIRST_PARTY_ORIGIN
        || scope
            .allowed_namespaces()
            .is_none_or(|allowed| allowed.contains(origin))
}

fn uri_allowed(uri: &str, scope: &ToolScope) -> Result<bool, ToolError> {
    let parsed = parse_skill_uri(uri).map_err(|error| ToolError::InvalidParam {
        message: error.to_string(),
        param: "uri".to_string(),
    })?;
    Ok(origin_allowed(parsed.origin(), scope))
}

fn tags(frontmatter: &serde_json::Map<String, Value>) -> Vec<String> {
    match frontmatter.get("tags") {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(Value::String(value)) => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

impl CodeModeSkillProvider for CanonicalCodeModeSkillProvider {
    fn list<'a>(
        &'a self,
        caller: &'a CodeModeCaller,
        scope: &'a ToolScope,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<Vec<CodeModeSkillSummary>, ToolError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let context = context_for(caller)?;
            let value = crate::dispatch::skills::dispatch_with_context(
                &context,
                "skills.list",
                json!({ "limit": 500 }),
            )
            .await?;
            let skills = value
                .get("skills")
                .and_then(Value::as_array)
                .ok_or_else(|| ToolError::Sdk {
                    sdk_kind: "internal_error".to_string(),
                    message: "canonical Skills list returned an invalid envelope".to_string(),
                })?;
            let mut summaries = Vec::with_capacity(skills.len());
            for skill in skills {
                let Some(origin) = skill.get("origin").and_then(Value::as_str) else {
                    continue;
                };
                if !origin_allowed(origin, scope) {
                    continue;
                }
                let Some(uri) = skill.get("uri").and_then(Value::as_str) else {
                    continue;
                };
                let name = skill
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let description = skill
                    .get("description")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string);
                let skill_tags = skill
                    .get("frontmatter")
                    .and_then(Value::as_object)
                    .map(tags)
                    .unwrap_or_default();
                summaries.push(CodeModeSkillSummary {
                    uri: uri.to_string(),
                    name,
                    description,
                    tags: skill_tags,
                });
            }
            Ok(summaries)
        })
    }

    fn get<'a>(
        &'a self,
        uri: &'a str,
        caller: &'a CodeModeCaller,
        scope: &'a ToolScope,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            if !uri_allowed(uri, scope)? {
                return Err(ToolError::Forbidden {
                    message: "Skill origin is outside this Code Mode scope".to_string(),
                    required_scopes: Vec::new(),
                });
            }
            let context = context_for(caller)?;
            crate::dispatch::skills::dispatch_with_context(
                &context,
                "skills.get",
                json!({ "uri": uri }),
            )
            .await
        })
    }

    fn read<'a>(
        &'a self,
        uri: &'a str,
        caller: &'a CodeModeCaller,
        scope: &'a ToolScope,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            if !uri_allowed(uri, scope)? {
                return Err(ToolError::Forbidden {
                    message: "Skill origin is outside this Code Mode scope".to_string(),
                    required_scopes: Vec::new(),
                });
            }
            let context = context_for(caller)?;
            crate::dispatch::skills::dispatch_with_context(
                &context,
                "skills.read",
                json!({ "uri": uri }),
            )
            .await
        })
    }
}
