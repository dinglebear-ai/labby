use serde_json::Value;

use crate::dispatch::error::ToolError;

pub(super) const DEFAULT_LIMIT: usize = 200;
pub(super) const MAX_LIMIT: usize = 1_000;
pub(super) const DEFAULT_SCAN_BYTES: u64 = 8 * 1024 * 1024;
pub(super) const MAX_SCAN_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(super) struct QueryParams {
    pub limit: usize,
    pub level: Option<String>,
    pub target: Option<String>,
    pub service: Option<String>,
    pub action: Option<String>,
    pub kind: Option<String>,
    pub query: Option<String>,
    pub file: Option<String>,
    pub max_scan_bytes: u64,
}

pub(super) fn parse(params: &Value) -> Result<QueryParams, ToolError> {
    Ok(QueryParams {
        limit: optional_usize(params, "limit")?
            .unwrap_or(DEFAULT_LIMIT)
            .clamp(1, MAX_LIMIT),
        level: optional_non_empty(params, "level")?.map(|level| level.to_ascii_uppercase()),
        target: optional_non_empty(params, "target")?.map(str::to_string),
        service: optional_non_empty(params, "service")?.map(str::to_string),
        action: optional_non_empty(params, "action")?.map(str::to_string),
        kind: optional_non_empty(params, "kind")?.map(str::to_string),
        query: optional_non_empty(params, "query")?.map(str::to_string),
        file: optional_non_empty(params, "file")?.map(str::to_string),
        max_scan_bytes: optional_u64(params, "max_scan_bytes")?
            .unwrap_or(DEFAULT_SCAN_BYTES)
            .clamp(1, MAX_SCAN_BYTES),
    })
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
