#![allow(dead_code)]

//! Surface-neutral redaction helpers for logs and observability.
//!
//! This module is the charter home for all redaction/sanitization helpers:
//! sensitive-key masking, URL/stdio-arg redaction, secret-like-segment
//! redaction, model-facing text sanitization, and JSON-tree trace redaction.
//! `crate::agent_error` re-exports the sanitize/secret helpers for existing
//! import paths.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value, json};
use url::Url;

/// Best-effort patterns for common secret shapes: provider API keys, JWTs,
/// Tailscale auth keys, `Bearer` authorization values, PEM private-key blocks,
/// and URL-embedded credentials (`scheme://user:pass@`).
///
/// This is defense-in-depth, not a guarantee. Novel or provider-specific token
/// formats pass through unrecognized, and a PEM body split across lines is only
/// caught from its `-----BEGIN` header onward. Never treat text that survived
/// this filter as safe to echo verbatim; avoid placing secrets in error text in
/// the first place.
static SECRET_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:sk-[A-Za-z0-9_-]{20,}|ghp_[A-Za-z0-9]{36}|github_pat_[A-Za-z0-9_]{82}|glpat-[A-Za-z0-9_-]{20}|xox[bp]-[A-Za-z0-9-]+|tskey-[A-Za-z0-9-]+|eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+|(?i:bearer)[ \t]+[A-Za-z0-9._~+/-]{8,}=*|-----BEGIN[A-Z ]*PRIVATE KEY-----[\s\S]*?(?:-----END[A-Z ]*PRIVATE KEY-----|$)|[A-Za-z][A-Za-z0-9+.-]*://[^/\s:@]+:[^/\s@]+@)",
    )
    .expect("secret regex is valid")
});

/// Marker appended by [`sanitize_error_text`] when the length cap or the
/// bounded inspection window dropped input.
pub const SANITIZE_TRUNCATION_MARKER: &str = " …[truncated]";

/// Slice `input` to a bounded inspection window BEFORE the retain / replace /
/// redact passes run, so a multi-megabyte upstream payload cannot force
/// several full-string passes whose output is then discarded by the final
/// character cap (amplification DoS). A `char` is at most four bytes, so a
/// window of `max_len * 4` bytes always contains at least `max_len` characters
/// when the input has them. The cut respects UTF-8 char boundaries.
fn bounded_window(input: &str, max_len: usize) -> &str {
    let cap = max_len.saturating_mul(4);
    if input.len() <= cap {
        return input;
    }
    let mut end = cap;
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    &input[..end]
}

#[must_use]
pub fn sanitize_log_text(input: &str, max_len: usize) -> String {
    sanitize_log_line(input, max_len).0
}

/// Shared single-line sanitizer. Returns the sanitized text plus whether the
/// window or the character cap dropped input, so multi-line callers can append
/// a truncation marker without recounting characters.
fn sanitize_log_line(input: &str, max_len: usize) -> (String, bool) {
    let window = bounded_window(input, max_len);
    let mut sanitized = window.to_string();
    sanitized.retain(|ch| {
        !matches!(
            ch,
            '\u{0000}'..='\u{001F}'
                | '\u{007F}'..='\u{009F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
        )
    });
    for marker in ["<system>", "[INST]", "###", "<<"] {
        sanitized = sanitized.replace(marker, "");
    }
    let redacted = redact_secret_like_segments(&sanitized);
    let mut chars = redacted.chars();
    let output: String = chars.by_ref().take(max_len).collect();
    let capped = chars.next().is_some() || window.len() < input.len();
    (output, capped)
}

/// Sanitize multiline model-facing diagnostics while preserving line breaks.
///
/// When the character cap (or the bounded inspection window) drops input, the
/// output ends with [`SANITIZE_TRUNCATION_MARKER`] so downstream readers know
/// evidence was cut rather than complete.
#[must_use]
pub fn sanitize_error_text(input: &str, max_len: usize) -> String {
    let window = bounded_window(input, max_len);
    let mut truncated = window.len() < input.len();
    let mut output = String::new();
    // Running character count instead of an O(n²) `output.chars().count()`
    // recount per line.
    let mut output_chars = 0usize;
    let mut lines = window.lines().peekable();
    let mut first = true;
    while let Some(line) = lines.next() {
        if !first {
            output.push('\n');
            output_chars += 1;
        }
        first = false;
        let (sanitized, capped) = sanitize_log_line(line, max_len);
        truncated |= capped;
        output_chars += sanitized.chars().count();
        output.push_str(&sanitized);
        if output_chars >= max_len {
            truncated |= output_chars > max_len || lines.peek().is_some();
            break;
        }
    }
    let mut output: String = output.chars().take(max_len).collect();
    if truncated {
        output.push_str(SANITIZE_TRUNCATION_MARKER);
    }
    output
}

/// Redact secret-shaped tokens from free-form text.
///
/// First pass: whitespace-split prefix heuristic (fast path for standalone
/// tokens). Second pass: [`struct@SECRET_REGEX`] catches embedded secrets (e.g.
/// header values, `Bearer` tokens, PEM blocks, URL credentials).
///
/// The `tskey-` prefix (Tailscale auth keys) was folded in from the setup
/// dispatch copies when they were consolidated onto this helper.
#[must_use]
pub fn redact_secret_like_segments(input: &str) -> String {
    let after_split = input
        .split_whitespace()
        .map(|segment| {
            let looks_secret = segment.starts_with("sk-")
                || segment.starts_with("ghp_")
                || segment.starts_with("github_pat_")
                || segment.starts_with("glpat-")
                || segment.starts_with("xoxb-")
                || segment.starts_with("xoxp-")
                || segment.starts_with("tskey-")
                || segment.starts_with("eyJ");
            if looks_secret {
                "[REDACTED]".to_string()
            } else {
                segment.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    SECRET_REGEX
        .replace_all(&after_split, "[REDACTED]")
        .into_owned()
}

/// Returns `true` for keys whose values must be masked in logs/observability.
///
/// The exact-match list also carries three ACP-origin keys that are NOT
/// credentials in the usual sense but are masked deliberately:
///
/// - `code` — ACP OAuth authorization codes flow through stdio args under this
///   key; an unmasked auth code is exchangeable for tokens.
/// - `cwd` — ACP session working directories leak the OS username and local
///   filesystem layout into shared logs.
/// - `terminal_id` — ACP terminal handles correlate a log line to a live
///   session; treated as sensitive session state.
///
/// Do not remove these without re-checking the ACP stdio redaction path — they
/// look innocuous but are intentional.
///
/// The broad `_key` suffix is kept (fail-safe: under-redaction is a security
/// risk, so any unknown `*_key` is masked) but a small allowlist of
/// known non-secret `_key` keys is carved out to stop the false positives the
/// review flagged: `sort_key`, `cache_key`, `idempotency_key`, `partition_key`,
/// `primary_key`. These are ordering/lookup/identity keys, never credentials.
/// The genuine credential cases remain covered by the
/// `_secret` / `_token` / `_password` / `api_key` arms regardless.
pub fn is_sensitive_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase().replace('-', "_");

    // Known non-secret keys that happen to end in `_key`. Kept narrow and
    // conservative — only add a key here when it is unambiguously NOT a
    // credential, since masking a real secret is the safer failure mode.
    if matches!(
        normalized.as_str(),
        "sort_key" | "cache_key" | "idempotency_key" | "partition_key" | "primary_key"
    ) {
        return false;
    }

    matches!(
        normalized.as_str(),
        "token"
            | "access_token"
            | "id_token"
            | "refresh_token"
            | "apikey"
            | "api_key"
            | "password"
            | "passwd"
            | "secret"
            | "client_secret"
            | "authorization"
            | "bearer"
            | "session"
            | "session_id"
            | "cookie"
            | "code"
            | "cwd"
            | "terminal_id"
    ) || normalized.ends_with("_token")
        || normalized.ends_with("_secret")
        || normalized.ends_with("_password")
        || normalized.ends_with("_key")
}

pub fn redact_url(url: &str) -> String {
    match Url::parse(url) {
        Ok(parsed) => redact_parsed_url(parsed),
        Err(_) => "[invalid-url-redacted]".to_string(),
    }
}

pub fn redact_stdio_value(value: &str) -> String {
    if let Some((key, _)) = value.split_once('=')
        && is_sensitive_key(key)
    {
        return format!("{key}=[redacted]");
    }

    if let Some(flag) = value.strip_prefix("--") {
        let (key, _) = flag.split_once('=').map_or((flag, ""), |(k, v)| (k, v));
        if is_sensitive_key(key) {
            return format!("--{key}=[redacted]");
        }
    }

    value.to_string()
}

pub fn redact_stdio_args(args: &[String]) -> Vec<String> {
    let mut redacted = Vec::with_capacity(args.len());
    let mut redact_next = false;

    for arg in args {
        if redact_next {
            redacted.push("[redacted]".to_string());
            redact_next = false;
            continue;
        }

        let is_sensitive_flag = arg
            .strip_prefix("--")
            .map(|value| value.split_once('=').map_or(value, |(key, _)| key))
            .is_some_and(is_sensitive_key);

        if is_sensitive_flag && !arg.contains('=') {
            redacted.push(arg.clone());
            redact_next = true;
            continue;
        }

        redacted.push(redact_stdio_value(arg));
    }

    redacted
}

pub fn redact_upstream_resource_uri(uri: &str) -> String {
    let Some(rest) = uri.strip_prefix("lab://upstream/") else {
        return redact_url(uri);
    };
    let Some(slash_pos) = rest.find('/') else {
        return "lab://upstream/[redacted]".to_string();
    };
    let upstream_name = &rest[..slash_pos];
    let original_uri = &rest[slash_pos + 1..];
    // Preserve non-sensitive pagination/id query params so observability can
    // still distinguish resources; only `is_sensitive_key` entries are masked.
    let redacted_original = redact_uri_or_path(original_uri);
    format!("lab://upstream/{upstream_name}/{redacted_original}")
}

fn redact_uri_or_path(value: &str) -> String {
    if let Ok(parsed) = Url::parse(value) {
        return redact_parsed_url(parsed);
    }
    let (path, query) = match value.split_once('?') {
        // Strip any `#fragment` from BOTH the path and the query so fragment
        // content cannot survive into redacted output (under-redaction guard).
        Some((path, rest)) => (
            path.split('#').next().unwrap_or(path),
            Some(rest.split('#').next().unwrap_or(rest)),
        ),
        None => (value.split('#').next().unwrap_or(value), None),
    };
    match query.map(redact_query_pairs) {
        Some(q) if !q.is_empty() => format!("{path}?{q}"),
        _ => path.to_string(),
    }
}

fn redact_parsed_url(mut parsed: Url) -> String {
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    let redacted_query = parsed.query().map(redact_query_pairs);
    parsed.set_query(redacted_query.as_deref());
    parsed.set_fragment(None);
    parsed.to_string()
}

fn redact_query_pairs(query: &str) -> String {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').map_or((pair, ""), |(k, v)| (k, v));
            if is_sensitive_key(key) {
                format!("{key}=[redacted]")
            } else if value.is_empty() {
                key.to_string()
            } else {
                format!("{key}={value}")
            }
        })
        .collect::<Vec<_>>()
        .join("&")
}

const TRACE_REDACTED: &str = "[redacted]";
const TRACE_TRUNCATED_STRING: &str = "[truncated]";
const TRACE_MAX_DEPTH: usize = 16;
const TRACE_MAX_COLLECTION_ITEMS: usize = 64;
const TRACE_MAX_STRING_CHARS: usize = 512;

/// Redact sensitive keys/values in a JSON tree and cap the serialized size.
///
/// Canonical implementation shared by the Code Mode trace path
/// (`labby_codemode` re-exports it for compatibility) and hosts that redact
/// journaled args/results with the same secret-key dictionary, keeping one
/// redaction implementation across storage, traces, and envelopes.
#[must_use]
pub fn redact_trace_value(value: &Value, max_bytes: usize) -> Value {
    let redacted = redact_json_value(value, 0);
    let size = serde_json::to_vec(&redacted)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX);
    if size <= max_bytes {
        return redacted;
    }

    json!({
        "truncated": true,
        "reason": "redacted_params_exceeded_cap",
        "original_size_bytes": size,
        "max_size_bytes": max_bytes,
    })
}

fn redact_json_value(value: &Value, depth: usize) -> Value {
    if depth >= TRACE_MAX_DEPTH {
        return json!({
            "truncated": true,
            "reason": "max_depth_exceeded",
        });
    }

    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
        Value::String(s) => Value::String(redact_json_string(s)),
        Value::Array(items) => {
            let mut out = items
                .iter()
                .take(TRACE_MAX_COLLECTION_ITEMS)
                .map(|item| redact_json_value(item, depth + 1))
                .collect::<Vec<_>>();
            if items.len() > TRACE_MAX_COLLECTION_ITEMS {
                out.push(json!({
                    "truncated": true,
                    "reason": "array_item_limit_exceeded",
                    "omitted": items.len() - TRACE_MAX_COLLECTION_ITEMS,
                }));
            }
            Value::Array(out)
        }
        Value::Object(map) => {
            let mut out = Map::new();
            for (idx, (key, child)) in map.iter().enumerate() {
                if idx >= TRACE_MAX_COLLECTION_ITEMS {
                    out.insert(
                        "_truncated".to_string(),
                        json!({
                            "reason": "object_key_limit_exceeded",
                            "omitted": map.len() - TRACE_MAX_COLLECTION_ITEMS,
                        }),
                    );
                    break;
                }
                if is_sensitive_key(key) {
                    out.insert(key.clone(), Value::String(TRACE_REDACTED.to_string()));
                } else {
                    out.insert(key.clone(), redact_json_value(child, depth + 1));
                }
            }
            Value::Object(out)
        }
    }
}

fn redact_json_string(value: &str) -> String {
    if looks_sensitive_value(value) {
        return TRACE_REDACTED.to_string();
    }

    let url_redacted = redact_url_like(value);
    truncate_trace_string(&url_redacted)
}

fn redact_url_like(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        return redact_url(value);
    }
    value.to_string()
}

fn truncate_trace_string(value: &str) -> String {
    let mut chars = value.chars();
    let prefix = chars
        .by_ref()
        .take(TRACE_MAX_STRING_CHARS)
        .collect::<String>();
    if chars.next().is_some() {
        format!(
            "{prefix}{TRACE_TRUNCATED_STRING} ({} chars)",
            value.chars().count()
        )
    } else {
        value.to_string()
    }
}

fn looks_sensitive_value(value: &str) -> bool {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();

    lower.starts_with("bearer ")
        || lower.starts_with("basic ")
        || lower.contains("-----begin ")
        || lower.contains("authorization:")
        || lower.contains("cookie:")
        || looks_like_jwt(trimmed)
        || looks_like_sensitive_assignment(trimmed)
        || looks_like_base64_blob(trimmed)
}

fn looks_like_sensitive_assignment(value: &str) -> bool {
    value.lines().any(|line| {
        let trimmed = line.trim();
        let Some((key, _)) = trimmed.split_once('=') else {
            return false;
        };
        is_sensitive_key(key.trim_start_matches("--"))
    })
}

fn looks_like_jwt(value: &str) -> bool {
    let parts = value.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| part.len() >= 10 && part.chars().all(is_base64url_char))
}

fn looks_like_base64_blob(value: &str) -> bool {
    value.len() >= 160 && value.chars().all(is_base64ish_char)
}

fn is_base64url_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_')
}

fn is_base64ish_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '=' | '-' | '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_sensitive_key_still_masks_real_secrets() {
        for key in [
            "api_key",
            "auth_token",
            "password",
            "client_secret",
            "service_api_key",
            "signing_secret_key",
            "tls_private_key",
            "code",
            "cwd",
            "terminal_id",
        ] {
            assert!(is_sensitive_key(key), "expected `{key}` to be sensitive");
        }
    }

    #[test]
    fn is_sensitive_key_allows_non_secret_key_suffixes() {
        for key in [
            "sort_key",
            "cache_key",
            "idempotency_key",
            "partition_key",
            "primary_key",
        ] {
            assert!(
                !is_sensitive_key(key),
                "expected `{key}` to NOT be sensitive"
            );
        }
    }

    #[test]
    fn redact_stdio_args_masks_split_form_secret_flags() {
        let args = vec![
            "npx".to_string(),
            "--api-key".to_string(),
            "super-secret".to_string(),
            "--token=abc123".to_string(),
        ];

        assert_eq!(
            redact_stdio_args(&args),
            vec![
                "npx".to_string(),
                "--api-key".to_string(),
                "[redacted]".to_string(),
                "--token=[redacted]".to_string(),
            ]
        );
    }

    #[test]
    fn redact_url_masks_credentials_and_sensitive_query_values() {
        assert_eq!(
            redact_url("http://user:pass@example.com/callback?token=secret&mode=1"),
            "http://example.com/callback?token=[redacted]&mode=1"
        );
    }

    #[test]
    fn redact_upstream_resource_uri_masks_embedded_credentials() {
        assert_eq!(
            redact_upstream_resource_uri(
                "lab://upstream/demo/https://user:pass@example.com/path?token=secret"
            ),
            "lab://upstream/demo/https://example.com/path?token=[redacted]"
        );
    }

    #[test]
    fn redact_upstream_resource_uri_preserves_non_sensitive_query_params() {
        assert_eq!(
            redact_upstream_resource_uri(
                "lab://upstream/demo/https://example.com/items?page=2&limit=50"
            ),
            "lab://upstream/demo/https://example.com/items?page=2&limit=50"
        );
    }

    #[test]
    fn redact_upstream_resource_uri_mixed_query_keys() {
        assert_eq!(
            redact_upstream_resource_uri(
                "lab://upstream/demo/https://example.com/items?page=2&api_key=abc"
            ),
            "lab://upstream/demo/https://example.com/items?page=2&api_key=[redacted]"
        );
    }

    #[test]
    fn redact_upstream_resource_uri_strips_fragment_after_query() {
        // A `#fragment` following a query string must not survive into the
        // redacted output (it could carry sensitive data into logs).
        assert_eq!(
            redact_upstream_resource_uri("lab://upstream/demo/items?page=2#token=leak"),
            "lab://upstream/demo/items?page=2"
        );
    }

    // ── Sanitize / secret-like-segment helpers (moved from agent_error) ──────

    #[test]
    fn sanitize_small_input_is_unchanged() {
        let input = "line one\nline two: all clear";
        assert_eq!(sanitize_error_text(input, 4096), input);
        assert_eq!(sanitize_log_text("plain text", 4096), "plain text");
    }

    #[test]
    fn sanitize_error_text_caps_multi_megabyte_input_and_marks_truncation() {
        // 5 MiB of secret-free text; before the bounded window landed this ran
        // ~8 full passes over the whole payload before the 8 KiB cap applied.
        let input = "x".repeat(5 * 1024 * 1024);
        let output = sanitize_error_text(&input, 8 * 1024);
        assert!(output.ends_with(SANITIZE_TRUNCATION_MARKER));
        let body = output.trim_end_matches(SANITIZE_TRUNCATION_MARKER);
        assert_eq!(body.chars().count(), 8 * 1024);

        // Multi-line variant: many lines, each within cap, total far above it.
        let input = "line of text\n".repeat(1024 * 1024);
        let output = sanitize_error_text(&input, 8 * 1024);
        assert!(output.ends_with(SANITIZE_TRUNCATION_MARKER));
        assert!(output.chars().count() <= 8 * 1024 + SANITIZE_TRUNCATION_MARKER.chars().count());
    }

    #[test]
    fn sanitize_exact_cap_input_gets_no_marker() {
        let input = "y".repeat(4096);
        let output = sanitize_error_text(&input, 4096);
        assert_eq!(output, input);
    }

    #[test]
    fn redacts_bearer_pem_and_url_credentials() {
        let bearer = redact_secret_like_segments("Authorization: Bearer abcdef1234567890");
        assert!(!bearer.contains("abcdef1234567890"), "{bearer}");
        assert!(bearer.contains("[REDACTED]"));

        let pem = redact_secret_like_segments(
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA7\n-----END RSA PRIVATE KEY-----",
        );
        assert!(!pem.contains("MIIEowIBAAKCAQEA7"), "{pem}");
        assert!(pem.contains("[REDACTED]"));

        let url = redact_secret_like_segments("connect postgres://admin:hunter2@db.local:5432/app");
        assert!(!url.contains("hunter2"), "{url}");
        assert!(url.contains("[REDACTED]"));
        assert!(url.contains("db.local"));
    }

    #[test]
    fn redacts_tailscale_auth_keys() {
        // `tskey-` was folded in from the setup dispatch copies when they were
        // consolidated onto this helper — both standalone and embedded forms.
        let standalone = redact_secret_like_segments("joined with tskey-auth-abc123CNTRL");
        assert!(!standalone.contains("tskey-auth"), "{standalone}");
        assert!(standalone.contains("[REDACTED]"));

        let embedded = redact_secret_like_segments("TS_AUTHKEY=tskey-auth-abc123CNTRL done");
        assert!(!embedded.contains("tskey-auth"), "{embedded}");
    }

    // ── Trace-value redaction (moved from labby-codemode trace) ──────────────

    #[test]
    fn trace_redacts_nested_sensitive_keys_and_values() {
        let raw = serde_json::json!({
            "query": "matrix",
            "nested": {
                "authorization": "Bearer secret-token",
                "items": [
                    {"api_key": "sk-secret"},
                    "https://user:pass@example.com/path?token=secret&page=2",
                    "OPENAI_API_KEY=sk-secret"
                ]
            }
        });

        let redacted = redact_trace_value(&raw, 4096);
        let serialized = redacted.to_string();

        assert_eq!(redacted["query"], serde_json::json!("matrix"));
        assert_eq!(
            redacted["nested"]["authorization"],
            serde_json::json!(TRACE_REDACTED)
        );
        assert_eq!(
            redacted["nested"]["items"][0]["api_key"],
            serde_json::json!(TRACE_REDACTED)
        );
        assert!(
            serialized.contains("token=[redacted]"),
            "credential URL query token must be redacted: {serialized}"
        );
        assert!(!serialized.contains("secret-token"));
        assert!(!serialized.contains("sk-secret"));
        assert!(!serialized.contains("user:pass"));
    }

    #[test]
    fn trace_redacts_sensitive_key_variants() {
        let raw = serde_json::json!({
            "token": "a",
            "secret": "b",
            "authorization": "c",
            "password": "d",
            "apikey": "e",
            "api_key": "f",
            "service-key": "g",
            "cookie": "h"
        });

        let redacted = redact_trace_value(&raw, 4096);
        for key in [
            "token",
            "secret",
            "authorization",
            "password",
            "apikey",
            "api_key",
            "service-key",
            "cookie",
        ] {
            assert_eq!(
                redacted[key],
                serde_json::json!(TRACE_REDACTED),
                "{key} must be redacted"
            );
        }
    }

    #[test]
    fn trace_caps_long_strings_and_large_objects_deterministically() {
        let long = "x".repeat(TRACE_MAX_STRING_CHARS + 100);
        let raw = serde_json::json!({
            "safe": long,
            "many": (0..200).map(|i| serde_json::json!({ "idx": i })).collect::<Vec<_>>()
        });

        let redacted = redact_trace_value(&raw, 512);
        let serialized = redacted.to_string();
        assert!(
            serialized.len() <= 512,
            "redacted params must respect byte cap, got {} bytes: {serialized}",
            serialized.len()
        );
        assert!(serialized.contains("redacted_params_exceeded_cap"));

        let string_capped = redact_trace_value(
            &serde_json::json!({"safe": "safe words ".repeat(TRACE_MAX_STRING_CHARS / 5)}),
            4096,
        );
        assert!(
            string_capped["safe"]
                .as_str()
                .expect("string")
                .contains(TRACE_TRUNCATED_STRING)
        );
    }
}
