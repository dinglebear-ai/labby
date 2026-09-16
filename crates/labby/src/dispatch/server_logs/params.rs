use serde_json::Value;

use crate::dispatch::error::ToolError;

pub(super) const DEFAULT_LIMIT: usize = 200;
pub(super) const MAX_LIMIT: usize = 1_000;
pub(super) const DEFAULT_SCAN_BYTES: u64 = 8 * 1024 * 1024;
pub(super) const MAX_SCAN_BYTES: u64 = 64 * 1024 * 1024;
const MAX_LEVEL_FILTERS: usize = 16;

#[derive(Debug, Clone)]
pub(super) struct QueryParams {
    pub limit: usize,
    pub level: Option<String>,
    pub levels: Vec<String>,
    pub target: Option<String>,
    pub service: Option<String>,
    pub action: Option<String>,
    pub kind: Option<String>,
    pub query: Option<String>,
    pub file: Option<String>,
    pub max_scan_bytes: u64,
    pub stop_after_limit: bool,
    pub correlated_only: bool,
}

pub(super) fn parse(params: &Value) -> Result<QueryParams, ToolError> {
    let level = optional_non_empty(params, "level")?.map(|level| level.to_ascii_uppercase());
    let levels = optional_levels(params)?;
    if level.is_some() && params.get("levels").is_some_and(|value| !value.is_null()) {
        return Err(ToolError::InvalidParam {
            message: "parameters `level` and `levels` are mutually exclusive".to_string(),
            param: "levels".to_string(),
        });
    }
    Ok(QueryParams {
        limit: optional_usize(params, "limit")?
            .unwrap_or(DEFAULT_LIMIT)
            .clamp(1, MAX_LIMIT),
        level,
        levels,
        target: optional_non_empty(params, "target")?.map(str::to_string),
        service: optional_non_empty(params, "service")?.map(str::to_string),
        action: optional_non_empty(params, "action")?.map(str::to_string),
        kind: optional_non_empty(params, "kind")?.map(str::to_string),
        query: optional_non_empty(params, "query")?.map(str::to_string),
        file: optional_non_empty(params, "file")?.map(str::to_string),
        max_scan_bytes: optional_u64(params, "max_scan_bytes")?
            .unwrap_or(DEFAULT_SCAN_BYTES)
            .clamp(1, MAX_SCAN_BYTES),
        stop_after_limit: optional_bool(params, "stop_after_limit")?.unwrap_or(false),
        correlated_only: optional_bool(params, "correlated_only")?.unwrap_or(false),
    })
}

fn optional_levels(params: &Value) -> Result<Vec<String>, ToolError> {
    let Some(value) = params.get("levels") else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    let Value::Array(values) = value else {
        return Err(ToolError::InvalidParam {
            message: "parameter `levels` must be an array of strings".to_string(),
            param: "levels".to_string(),
        });
    };
    if values.len() > MAX_LEVEL_FILTERS {
        return Err(ToolError::InvalidParam {
            message: format!("parameter `levels` accepts at most {MAX_LEVEL_FILTERS} values"),
            param: "levels".to_string(),
        });
    }
    let mut levels = Vec::with_capacity(values.len());
    for value in values {
        let Some(level) = value.as_str() else {
            return Err(ToolError::InvalidParam {
                message: "parameter `levels` must contain only strings".to_string(),
                param: "levels".to_string(),
            });
        };
        let level = level.trim();
        if level.is_empty() {
            return Err(ToolError::InvalidParam {
                message: "parameter `levels` must not contain empty strings".to_string(),
                param: "levels".to_string(),
            });
        }
        levels.push(level.to_ascii_uppercase());
    }
    levels.sort_unstable();
    levels.dedup();
    Ok(levels)
}

fn optional_non_empty<'a>(params: &'a Value, key: &str) -> Result<Option<&'a str>, ToolError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.trim().is_empty() => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.trim())),
        Some(_) => Err(ToolError::InvalidParam {
            message: format!("parameter `{key}` must be a string"),
            param: key.to_string(),
        }),
    }
}

fn optional_usize(params: &Value, key: &str) -> Result<Option<usize>, ToolError> {
    optional_u64(params, key).map(|value| value.map(|n| n as usize))
}

fn optional_bool(params: &Value, key: &str) -> Result<Option<bool>, ToolError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(ToolError::InvalidParam {
            message: format!("parameter `{key}` must be a boolean"),
            param: key.to_string(),
        }),
    }
}

fn optional_u64(params: &Value, key: &str) -> Result<Option<u64>, ToolError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(value)) => {
            value
                .as_u64()
                .map(Some)
                .ok_or_else(|| ToolError::InvalidParam {
                    message: format!("parameter `{key}` must be a positive integer"),
                    param: key.to_string(),
                })
        }
        Some(_) => Err(ToolError::InvalidParam {
            message: format!("parameter `{key}` must be an integer"),
            param: key.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn levels_are_bounded_normalized_and_deduplicated() {
        let parsed = parse(&json!({ "levels": [" warn ", "INFO", "warn"] })).expect("parse levels");

        assert_eq!(parsed.levels, ["INFO", "WARN"]);
    }

    #[test]
    fn singular_and_plural_level_filters_are_mutually_exclusive() {
        let error = parse(&json!({ "level": "INFO", "levels": ["WARN"] }))
            .expect_err("reject ambiguous level filters");

        assert!(matches!(error, ToolError::InvalidParam { ref param, .. } if param == "levels"));
    }

    #[test]
    fn levels_reject_more_than_the_bounded_limit() {
        let error = parse(&json!({ "levels": vec!["INFO"; MAX_LEVEL_FILTERS + 1] }))
            .expect_err("reject oversized levels");

        assert!(matches!(error, ToolError::InvalidParam { ref param, .. } if param == "levels"));
    }
}
