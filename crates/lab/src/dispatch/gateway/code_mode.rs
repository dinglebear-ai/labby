use serde::Serialize;
use serde_json::Value;

use crate::dispatch::error::ToolError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeModeToolId {
    pub raw: String,
    pub reference: CodeModeToolRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeModeToolRef {
    LabAction { service: String, action: String },
    UpstreamTool { upstream: String, tool: String },
}

impl CodeModeToolId {
    pub fn parse(raw: &str) -> Result<Self, ToolError> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(invalid_code_mode_id("Code Mode tool id must not be empty"));
        }

        if let Some(rest) = raw.strip_prefix("lab::") {
            let (service, action) = rest.split_once('.').ok_or_else(|| {
                invalid_code_mode_id("lab Code Mode ids must use lab::<service>.<action>")
            })?;
            if service.trim().is_empty() || action.trim().is_empty() {
                return Err(invalid_code_mode_id(
                    "lab Code Mode ids must include service and action",
                ));
            }
            return Ok(Self {
                raw: raw.to_string(),
                reference: CodeModeToolRef::LabAction {
                    service: service.trim().to_string(),
                    action: action.trim().to_string(),
                },
            });
        }

        if let Some(rest) = raw.strip_prefix("upstream::") {
            let (upstream, tool) = rest.split_once("::").ok_or_else(|| {
                invalid_code_mode_id("upstream Code Mode ids must use upstream::<upstream>::<tool>")
            })?;
            if upstream.trim().is_empty() || tool.trim().is_empty() {
                return Err(invalid_code_mode_id(
                    "upstream Code Mode ids must include upstream and tool",
                ));
            }
            return Ok(Self {
                raw: raw.to_string(),
                reference: CodeModeToolRef::UpstreamTool {
                    upstream: upstream.trim().to_string(),
                    tool: tool.trim().to_string(),
                },
            });
        }

        Err(invalid_code_mode_id(
            "Code Mode ids must start with lab:: or upstream::",
        ))
    }
}

#[must_use]
pub fn lab_action_id(service: &str, action: &str) -> String {
    format!("lab::{service}.{action}")
}

#[must_use]
pub fn upstream_tool_id(upstream: &str, tool: &str) -> String {
    format!("upstream::{upstream}::{tool}")
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CodeModeSearchCandidate {
    pub id: String,
    pub name: String,
    pub upstream: String,
    pub description: String,
    pub score: f32,
    pub schema_available: bool,
}

impl CodeModeSearchCandidate {
    #[must_use]
    pub fn lab_action(service: &str, action: &str, description: &str, score: f32) -> Self {
        Self {
            id: lab_action_id(service, action),
            name: action.to_string(),
            upstream: "lab".to_string(),
            description: description.to_string(),
            score,
            schema_available: true,
        }
    }

    #[must_use]
    pub fn upstream_tool(
        upstream: &str,
        tool: &str,
        description: &str,
        score: f32,
        schema: Option<Value>,
    ) -> Self {
        Self {
            id: upstream_tool_id(upstream, tool),
            name: tool.to_string(),
            upstream: upstream.to_string(),
            description: description.to_string(),
            score,
            schema_available: schema.is_some(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CodeModeSchemaResponse {
    pub id: String,
    pub kind: &'static str,
    pub name: String,
    pub upstream: String,
    pub schema: Value,
    pub schema_format: &'static str,
}

impl CodeModeSchemaResponse {
    #[must_use]
    pub fn lab_action(id: &str, action: &str, schema: Value) -> Self {
        Self {
            id: id.to_string(),
            kind: "lab_action",
            name: action.to_string(),
            upstream: "lab".to_string(),
            schema,
            schema_format: "lab_action_spec",
        }
    }

    #[must_use]
    pub fn upstream_tool(id: &str, upstream: &str, tool: &str, schema: Value) -> Self {
        Self {
            id: id.to_string(),
            kind: "upstream_tool",
            name: tool.to_string(),
            upstream: upstream.to_string(),
            schema,
            schema_format: "json_schema",
        }
    }
}

pub fn invalid_code_mode_id(message: impl Into<String>) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "invalid_code_mode_id".to_string(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{CodeModeSchemaResponse, CodeModeSearchCandidate, CodeModeToolId, CodeModeToolRef};

    #[test]
    fn parses_lab_action_id() {
        let parsed = CodeModeToolId::parse("lab::gateway.gateway.schema").unwrap();
        assert_eq!(
            parsed,
            CodeModeToolId {
                raw: "lab::gateway.gateway.schema".to_string(),
                reference: CodeModeToolRef::LabAction {
                    service: "gateway".to_string(),
                    action: "gateway.schema".to_string(),
                },
            }
        );
    }

    #[test]
    fn parses_upstream_tool_id() {
        let parsed = CodeModeToolId::parse("upstream::github::search_issues").unwrap();
        assert_eq!(
            parsed,
            CodeModeToolId {
                raw: "upstream::github::search_issues".to_string(),
                reference: CodeModeToolRef::UpstreamTool {
                    upstream: "github".to_string(),
                    tool: "search_issues".to_string(),
                },
            }
        );
    }

    #[test]
    fn rejects_invalid_ids() {
        for id in [
            "",
            "gateway.gateway.schema",
            "lab::gateway",
            "upstream::github",
            "upstream::::tool",
        ] {
            assert!(CodeModeToolId::parse(id).is_err(), "{id} should be invalid");
        }
    }

    #[test]
    fn builds_search_candidate_for_lab_action() {
        let candidate = CodeModeSearchCandidate::lab_action(
            "gateway",
            "gateway.schema",
            "Return gateway schema",
            10.0,
        );
        assert_eq!(candidate.id, "lab::gateway.gateway.schema");
        assert_eq!(candidate.upstream, "lab");
        assert_eq!(candidate.name, "gateway.schema");
        assert!(candidate.schema_available);
    }

    #[test]
    fn builds_search_candidate_for_upstream_tool() {
        let candidate = CodeModeSearchCandidate::upstream_tool(
            "github",
            "search_issues",
            "Search issues",
            8.5,
            Some(json!({"type": "object"})),
        );
        assert_eq!(candidate.id, "upstream::github::search_issues");
        assert_eq!(candidate.upstream, "github");
        assert_eq!(candidate.name, "search_issues");
        assert!(candidate.schema_available);
    }

    #[test]
    fn builds_lab_schema_response() {
        let response = CodeModeSchemaResponse::lab_action(
            "lab::gateway.gateway.schema",
            "gateway.schema",
            json!({"action": "gateway.schema"}),
        );
        assert_eq!(response.kind, "lab_action");
        assert_eq!(response.schema_format, "lab_action_spec");
    }

    #[test]
    fn builds_upstream_schema_response() {
        let response = CodeModeSchemaResponse::upstream_tool(
            "upstream::github::search_issues",
            "github",
            "search_issues",
            json!({"type": "object"}),
        );
        assert_eq!(response.kind, "upstream_tool");
        assert_eq!(response.schema_format, "json_schema");
    }
}
