//! In-memory per-label registry of loaded OpenAPI specs.
//!
//! Load-once-at-process-start (v1): no background refresh, no `ArcSwap`, no TTL.
//! Specs load concurrently with a per-spec timeout and a pre-parse body-size cap;
//! a spec that fails to load is omitted with a structured WARN and never blocks a
//! healthy spec (degraded boot).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::config::{OpenApiCredential, OpenApiProviderConfig, OpenApiSpecConfig, SpecSource};
use crate::error::OpenApiError;

const MAX_SPECS: usize = 10;
const MAX_OPERATIONS_PER_SPEC: usize = 200;
/// Reject spec documents larger than 4 MiB before parse.
const MAX_SPEC_BYTES: usize = 4 * 1024 * 1024;

/// A single dispatchable operation, fully resolved for outbound execution.
#[derive(Debug, Clone)]
pub struct OperationHandle {
    /// Raw operationId (allowlist + dispatch key).
    pub operation_id: String,
    /// HTTP method.
    pub method: reqwest::Method,
    /// Path template, e.g. `/users/{id}`.
    pub path_template: String,
    /// SSRF-validated base URL.
    pub base_url: url::Url,
    /// Optional server-side credential.
    pub credential: Option<OpenApiCredential>,
}

/// All operations for one spec, keyed by raw operationId.
#[derive(Debug, Clone)]
pub struct SpecEntry {
    /// operationId → handle.
    pub operations: HashMap<String, OperationHandle>,
}

/// Cheap-to-clone (`Arc`-backed) per-label registry.
#[derive(Clone, Default)]
pub struct OpenApiRegistry {
    inner: Arc<HashMap<String, SpecEntry>>,
}

impl OpenApiRegistry {
    /// Load all specs concurrently, each bounded by `per_spec_timeout`. A spec
    /// that fails validation, fetch, or parse is omitted with a WARN. Extra
    /// specs beyond `MAX_SPECS` are dropped with a WARN.
    pub async fn load(cfg: OpenApiProviderConfig, per_spec_timeout: Duration) -> Self {
        let total = cfg.specs.len();
        let specs: Vec<_> = cfg.specs.into_iter().take(MAX_SPECS).collect();
        if total > MAX_SPECS {
            tracing::warn!(
                service = "openapi",
                kept = MAX_SPECS,
                configured = total,
                "openapi: MAX_SPECS exceeded — extra specs dropped"
            );
        }
        let loads = specs.into_iter().map(|spec| async move {
            let label = spec.label.clone();
            match tokio::time::timeout(per_spec_timeout, load_one_spec(spec)).await {
                Ok(Ok(entry)) => Some((label, entry)),
                Ok(Err(e)) => {
                    tracing::warn!(service = "openapi", label = %label, kind = e.kind(),
                            "openapi spec omitted: load failed");
                    None
                }
                Err(_) => {
                    tracing::warn!(service = "openapi", label = %label, kind = "timeout",
                            "openapi spec omitted: load timed out");
                    None
                }
            }
        });
        let map: HashMap<_, _> = futures::future::join_all(loads)
            .await
            .into_iter()
            .flatten()
            .collect();
        Self {
            inner: Arc::new(map),
        }
    }

    /// Construct a registry directly from a label→entry map. TEST-ONLY: bypasses
    /// spec loading and the SSRF guard so dispatch logic can be exercised against
    /// a loopback mock server. Not part of the production API.
    #[cfg(test)]
    #[must_use]
    pub fn from_map_for_test(map: HashMap<String, SpecEntry>) -> Self {
        Self {
            inner: Arc::new(map),
        }
    }

    /// Sorted list of loaded labels.
    #[must_use]
    pub fn labels(&self) -> Vec<String> {
        let mut v: Vec<_> = self.inner.keys().cloned().collect();
        v.sort();
        v
    }

    /// Whether no specs loaded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Look up one operation. Unknown label → `UnknownInstance`; unknown op →
    /// `UnknownOperation`.
    ///
    /// # Errors
    /// Returns a structured [`OpenApiError`] for an unknown label or operation.
    pub fn operation(&self, label: &str, op: &str) -> Result<&OperationHandle, OpenApiError> {
        let entry = self
            .inner
            .get(label)
            .ok_or_else(|| OpenApiError::UnknownInstance {
                label: label.into(),
                valid: self.labels(),
            })?;
        entry
            .operations
            .get(op)
            .ok_or_else(|| OpenApiError::UnknownOperation {
                label: label.into(),
                operation_id: op.into(),
            })
    }
}

async fn load_one_spec(spec: OpenApiSpecConfig) -> Result<SpecEntry, OpenApiError> {
    let base_url = crate::ssrf::validate_base_url(&spec)?;
    let spec_json = fetch_spec_json(&spec.spec_source, &spec.label).await?;
    let descriptors =
        crate::convert::convert_spec(&spec.label, &spec_json, &spec.allowed_operations)?;
    let converted = descriptors.len();
    let mut operations = HashMap::new();
    for d in descriptors.into_iter().take(MAX_OPERATIONS_PER_SPEC) {
        operations.insert(
            d.operation_id.clone(),
            OperationHandle {
                operation_id: d.operation_id,
                method: d.method,
                path_template: d.path_template,
                base_url: base_url.clone(),
                credential: spec.credential.clone(),
            },
        );
    }
    if converted > MAX_OPERATIONS_PER_SPEC {
        tracing::warn!(
            service = "openapi",
            label = %spec.label,
            kept = MAX_OPERATIONS_PER_SPEC,
            converted,
            "openapi: MAX_OPERATIONS_PER_SPEC exceeded — extra operations dropped"
        );
    }
    if operations.is_empty() {
        // The spec fetched + parsed fine but nothing matched the allowlist. This
        // spec loads as present-but-empty: its JS shim is emitted yet every call
        // returns `unknown_action`. Surface it so a fat-fingered / forgotten
        // `allowed_operations` is diagnosable instead of silently rejecting.
        tracing::warn!(
            service = "openapi",
            label = %spec.label,
            allowed = spec.allowed_operations.len(),
            kind = "empty_allowlist",
            "openapi spec loaded but no operations matched the allowlist"
        );
    }
    Ok(SpecEntry { operations })
}

/// Fetch a spec document, capped at `MAX_SPEC_BYTES` before parse. A remote
/// `SpecSource::Url` is SSRF-validated (same canonical guard as `base_url`)
/// BEFORE any outbound request.
async fn fetch_spec_json(source: &SpecSource, label: &str) -> Result<String, OpenApiError> {
    match source {
        SpecSource::Url(url) => {
            crate::ssrf::validate_spec_url(label, url)?;
            crate::http::fetch_url_capped(url, MAX_SPEC_BYTES, label).await
        }
        SpecSource::Path(path) => read_path_capped(path, MAX_SPEC_BYTES, label).await,
    }
}

async fn read_path_capped(
    path: &std::path::Path,
    cap: usize,
    label: &str,
) -> Result<String, OpenApiError> {
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|_| OpenApiError::SpecParse {
            label: label.to_string(),
        })?;
    if bytes.len() > cap {
        return Err(OpenApiError::SpecTooLarge {
            label: label.to_string(),
        });
    }
    String::from_utf8(bytes).map_err(|_| OpenApiError::SpecParse {
        label: label.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SpecSource;

    const FIXTURE_SPEC: &str = r#"{
        "openapi": "3.0.0",
        "info": { "title": "Fixture", "version": "1.0.0" },
        "paths": {
            "/users/{id}": {
                "get": {
                    "operationId": "getUser",
                    "responses": { "200": { "description": "ok" } }
                }
            }
        }
    }"#;

    fn good_fixture_spec(label: &str) -> OpenApiSpecConfig {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("labby-openapi-fixture-{label}.json"));
        std::fs::write(&path, FIXTURE_SPEC).expect("write fixture");
        OpenApiSpecConfig {
            label: label.into(),
            spec_source: SpecSource::Path(path),
            base_url: "https://api.example.com".parse().unwrap(),
            allowed_operations: vec!["getUser".into()],
            credential: None,
        }
    }

    fn bad_spec(label: &str, base: &str) -> OpenApiSpecConfig {
        OpenApiSpecConfig {
            label: label.into(),
            spec_source: SpecSource::Path(std::env::temp_dir().join("nonexistent.json")),
            base_url: base.parse().unwrap(),
            allowed_operations: vec![],
            credential: None,
        }
    }

    #[tokio::test]
    async fn one_bad_spec_omitted_without_blocking_good_one() {
        let cfg = OpenApiProviderConfig {
            specs: vec![
                good_fixture_spec("goodlabel"),
                // SSRF-rejected at validate_base_url (RFC1918 literal).
                bad_spec("badlabel", "https://10.255.255.1"),
            ],
        };
        let started = std::time::Instant::now();
        let reg = OpenApiRegistry::load(cfg, Duration::from_secs(2)).await;
        assert!(reg.labels().contains(&"goodlabel".to_string()));
        assert!(!reg.labels().contains(&"badlabel".to_string()));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "concurrent + bounded"
        );
    }

    #[tokio::test]
    async fn good_spec_exposes_allowed_operation() {
        let cfg = OpenApiProviderConfig {
            specs: vec![good_fixture_spec("vendor")],
        };
        let reg = OpenApiRegistry::load(cfg, Duration::from_secs(2)).await;
        let op = reg.operation("vendor", "getUser").expect("op present");
        assert_eq!(op.method, reqwest::Method::GET);
        assert_eq!(op.path_template, "/users/{id}");
        assert_eq!(
            reg.operation("vendor", "nope").unwrap_err().kind(),
            "unknown_action"
        );
        assert_eq!(
            reg.operation("nope", "getUser").unwrap_err().kind(),
            "unknown_instance"
        );
    }
}
