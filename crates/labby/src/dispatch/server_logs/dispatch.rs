use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde::Serialize;
use serde_json::{Value, json};

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{action_schema, help_payload, require_str, to_json};
use crate::dispatch::redact::is_sensitive_key;
use crate::dispatch::server_logs::catalog::ACTIONS;
use crate::dispatch::server_logs::client::{LogFile, display_path, log_dir, log_files};
use crate::dispatch::server_logs::params::{QueryParams, parse};

pub async fn dispatch(action: &str, params: Value) -> Result<Value, ToolError> {
    match action {
        "help" => Ok(help_payload("server_logs", ACTIONS)),
        "schema" => {
            let a = require_str(&params, "action")?;
            action_schema(ACTIONS, a)
        }
        "server_logs.query" => to_json(query(parse(&params)?)?),
        unknown => Err(ToolError::UnknownAction {
            message: format!("unknown action `server_logs.{unknown}`"),
            valid: ACTIONS.iter().map(|a| a.name.to_string()).collect(),
            hint: None,
        }),
    }
}

#[derive(Debug, Serialize)]
struct QueryResult {
    kind: &'static str,
    log_dir: String,
    filters: AppliedFilters,
    files: Vec<FileSummary>,
    entries: Vec<LogEntry>,
    matched: usize,
    scanned_lines: usize,
    malformed_lines: usize,
    scanned_bytes: u64,
    max_scan_bytes: u64,
    truncated: bool,
}

#[derive(Debug, Serialize)]
struct AppliedFilters {
    limit: usize,
    level: Option<String>,
    target: Option<String>,
    service: Option<String>,
    action: Option<String>,
    kind: Option<String>,
    query: Option<String>,
    file: Option<String>,
}

#[derive(Debug, Serialize)]
struct FileSummary {
    name: String,
    path: String,
    bytes: u64,
    scanned_bytes: u64,
    modified_unix_ms: Option<u128>,
}

#[derive(Debug, Clone, Serialize)]
struct LogEntry {
    timestamp: Option<String>,
    level: Option<String>,
    target: Option<String>,
    message: Option<String>,
    service: Option<String>,
    action: Option<String>,
    kind: Option<String>,
    file: String,
    fields: Value,
}

fn query(params: QueryParams) -> Result<QueryResult, ToolError> {
    let dir = log_dir();
    query_from_dir(&dir, params)
}

fn query_from_dir(dir: &Path, params: QueryParams) -> Result<QueryResult, ToolError> {
    let files = log_files(&dir)?;
    let mut remaining_bytes = params.max_scan_bytes;
    let mut summaries = Vec::new();
    let mut entries = Vec::new();
    let mut scanned_lines = 0usize;
    let mut malformed_lines = 0usize;
    let mut matched_total = 0usize;
    let mut scanned_bytes = 0u64;
    let mut truncated = false;

    for file in files.iter().rev() {
        if remaining_bytes == 0 {
            truncated = true;
            break;
        }
        if !matches_file_filter(file, &params) {
            continue;
        }
        let bytes_to_read = remaining_bytes.min(file.bytes);
        remaining_bytes -= bytes_to_read;
        scanned_bytes += bytes_to_read;
        summaries.push(FileSummary {
            name: file.name.clone(),
            path: display_path(&file.path),
            bytes: file.bytes,
            scanned_bytes: bytes_to_read,
            modified_unix_ms: file.modified_unix_ms,
        });

        let text = read_tail(&file.path, file.bytes, bytes_to_read)?;
        for line in text.lines().rev() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            scanned_lines += 1;
            let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
                malformed_lines += 1;
                continue;
            };
            let Some(entry) = normalize_entry(&value, &file.name) else {
                malformed_lines += 1;
                continue;
            };
            if !entry_matches(&entry, &params) {
                continue;
            }
            matched_total += 1;
            if entries.len() < params.limit {
                entries.push(entry);
            }
        }
    }

    let returned_full_match_set = matched_total <= entries.len();
    Ok(QueryResult {
        kind: "server_logs",
        log_dir: display_path(dir),
        filters: AppliedFilters {
            limit: params.limit,
            level: params.level,
            target: params.target,
            service: params.service,
            action: params.action,
            kind: params.kind,
            query: params.query,
            file: params.file,
        },
        files: summaries,
        entries,
        matched: matched_total,
        scanned_lines,
        malformed_lines,
        scanned_bytes,
        max_scan_bytes: params.max_scan_bytes,
        truncated: truncated || !returned_full_match_set,
    })
}

fn read_tail(path: &Path, file_bytes: u64, bytes_to_read: u64) -> Result<String, ToolError> {
    let mut file = std::fs::File::open(path).map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to open server log file `{}`: {err}", path.display()),
    })?;
    let offset = file_bytes.saturating_sub(bytes_to_read);
    file.seek(SeekFrom::Start(offset))
        .map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to seek server log file `{}`: {err}", path.display()),
        })?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to read server log file `{}`: {err}", path.display()),
    })?;
    if offset > 0 {
        if let Some(index) = bytes.iter().position(|byte| *byte == b'\n') {
            bytes.drain(..=index);
        } else {
            bytes.clear();
        }
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn normalize_entry(value: &Value, file: &str) -> Option<LogEntry> {
    let object = value.as_object()?;
    let fields = object
        .get("fields")
        .filter(|fields| fields.is_object())
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    let redacted_fields = redact_fields(fields);
    Some(LogEntry {
        timestamp: string_field(value, "timestamp"),
        level: string_field(value, "level").map(|level| level.to_ascii_uppercase()),
        target: string_field(value, "target"),
        message: string_field(&redacted_fields, "message"),
        service: string_field(&redacted_fields, "service"),
        action: string_field(&redacted_fields, "action"),
        kind: string_field(&redacted_fields, "kind"),
        file: file.to_string(),
        fields: redacted_fields,
    })
}

fn redact_fields(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| {
                    if is_sensitive_key(&key) {
                        (key, json!("[redacted]"))
                    } else {
                        (key, redact_fields(value))
                    }
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.into_iter().map(redact_fields).collect()),
        other => other,
    }
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn matches_file_filter(file: &LogFile, params: &QueryParams) -> bool {
    params
        .file
        .as_deref()
        .is_none_or(|needle| contains_ci(&file.name, needle))
}

fn entry_matches(entry: &LogEntry, params: &QueryParams) -> bool {
    if let Some(level) = &params.level
        && entry.level.as_deref() != Some(level.as_str())
    {
        return false;
    }
    if !matches_optional(&entry.target, &params.target) {
        return false;
    }
    if !matches_optional(&entry.service, &params.service) {
        return false;
    }
    if !matches_optional(&entry.action, &params.action) {
        return false;
    }
    if !matches_optional(&entry.kind, &params.kind) {
        return false;
    }
    if let Some(query) = &params.query {
        let haystack = serde_json::to_string(entry).unwrap_or_default();
        if !contains_ci(&haystack, query) {
            return false;
        }
    }
    true
}

fn matches_optional(value: &Option<String>, filter: &Option<String>) -> bool {
    match filter.as_deref() {
        None => true,
        Some(needle) => value
            .as_deref()
            .is_some_and(|value| contains_ci(value, needle)),
    }
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_reads_filters_and_redacts_server_process_logs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log_path = dir.path().join("lab.2026-07-12.log");
        std::fs::write(
            &log_path,
            [
                r#"{"timestamp":"2026-07-12T00:00:01Z","level":"INFO","target":"labby::mcp","fields":{"message":"started","service":"gateway","action":"serve","token":"secret-value"}}"#,
                r#"{"timestamp":"2026-07-12T00:00:02Z","level":"ERROR","target":"labby::mcp","fields":{"message":"boom","service":"gateway","action":"read_resource","kind":"internal_error"}}"#,
            ]
            .join("\n"),
        )
        .expect("write log");

        let params = QueryParams {
            limit: 10,
            level: Some("INFO".to_string()),
            target: None,
            service: Some("gateway".to_string()),
            action: None,
            kind: None,
            query: Some("started".to_string()),
            file: None,
            max_scan_bytes: 1024 * 1024,
        };

        let result = query_from_dir(dir.path(), params).expect("query");

        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].message.as_deref(), Some("started"));
        assert_eq!(result.entries[0].fields["token"], json!("[redacted]"));
    }
}
