//! In-memory per-label registry of loaded OpenAPI specs.
//!
//! Load-once-at-process-start (v1): no background refresh, no `ArcSwap`, no TTL.
//! Specs load concurrently with a per-spec timeout and a pre-parse body-size cap;
//! a spec that fails to load is omitted with a structured WARN and never blocks a
//! healthy spec (degraded boot).

use std::collections::HashMap;
use std::path::Path;
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
    /// Optional server-side static credential.
    pub credential: Option<OpenApiCredential>,
    /// Optional OAuth upstream reference resolved per caller subject by the host.
    pub oauth_upstream: Option<String>,
}

/// All operations for one spec, keyed by raw operationId.
#[derive(Debug, Clone)]
pub struct SpecEntry {
    /// operationId → handle.
    pub operations: HashMap<String, OperationHandle>,
    #[cfg(feature = "mcp-registry")]
    pub(crate) mcp: crate::mcp_schema::Tools,
}

/// Cheap-to-clone (`Arc`-backed) per-label registry.
#[derive(Clone, Default)]
pub struct OpenApiRegistry {
    inner: Arc<HashMap<String, SpecEntry>>,
    warnings: Arc<Vec<String>>,
}

impl OpenApiRegistry {
    /// Load all specs concurrently, each bounded by `per_spec_timeout`. A spec
    /// that fails validation, fetch, or parse is omitted with a WARN. Extra
    /// specs beyond `MAX_SPECS` are dropped with a WARN.
    pub async fn load(cfg: OpenApiProviderConfig, per_spec_timeout: Duration) -> Self {
        Self::load_inner(cfg, per_spec_timeout, false).await
    }

    /// Load the standalone profile, projecting MCP metadata only for retained
    /// operations. Ordinary registry loads do not project MCP schemas.
    #[cfg(feature = "mcp-registry")]
    pub async fn load_mcp(cfg: OpenApiProviderConfig, per_spec_timeout: Duration) -> Self {
        Self::load_inner(cfg, per_spec_timeout, true).await
    }

    async fn load_inner(
        cfg: OpenApiProviderConfig,
        per_spec_timeout: Duration,
        project_mcp: bool,
    ) -> Self {
        let total = cfg.specs.len();
        let specs: Vec<_> = cfg.specs.into_iter().take(MAX_SPECS).collect();
        let mut warnings = Vec::new();
        if total > MAX_SPECS {
            let warning = format!(
                "{total} OpenAPI specs are configured but only {MAX_SPECS} can be loaded; remove or consolidate the extra specs"
            );
            warnings.push(warning);
            tracing::warn!(
                service = "openapi",
                kept = MAX_SPECS,
                configured = total,
                "openapi: MAX_SPECS exceeded — extra specs dropped"
            );
        }
        let loads = specs.into_iter().map(|spec| async move {
            let label = spec.label.clone();
            match tokio::time::timeout(per_spec_timeout, load_one_spec(spec, project_mcp)).await {
                Ok(Ok((entry, warnings))) => (Some((label, entry)), warnings),
                Ok(Err(e)) => {
                    let warning = format!("OpenAPI spec `{label}` is unavailable: {e}");
                    tracing::warn!(service = "openapi", label = %label, kind = e.kind(),
                            "openapi spec omitted: load failed");
                    (None, vec![warning])
                }
                Err(_) => {
                    let warning = format!(
                        "OpenAPI spec `{label}` did not load within {}s; verify the spec source is reachable and responsive",
                        per_spec_timeout.as_secs_f32()
                    );
                    tracing::warn!(service = "openapi", label = %label, kind = "timeout",
                            "openapi spec omitted: load timed out");
                    (None, vec![warning])
                }
            }
        });
        let mut map = HashMap::new();
        for (entry, mut load_warnings) in futures::future::join_all(loads).await {
            if let Some((label, spec)) = entry {
                map.insert(label, spec);
            }
            warnings.append(&mut load_warnings);
        }
        Self {
            inner: Arc::new(map),
            warnings: Arc::new(warnings),
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
            warnings: Arc::new(Vec::new()),
        }
    }

    /// Sorted list of loaded labels.
    #[must_use]
    pub fn labels(&self) -> Vec<String> {
        let mut v: Vec<_> = self.inner.keys().cloned().collect();
        v.sort();
        v
    }

    /// Operator-facing, scrubbed reasons configured OpenAPI capability was reduced.
    #[must_use]
    pub fn warnings(&self) -> &[String] {
        self.warnings.as_slice()
    }

    /// Whether no specs loaded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Whether at least one loaded operation resolves OAuth per caller subject.
    #[must_use]
    pub fn has_subject_scoped_operations(&self) -> bool {
        self.inner.values().any(|entry| {
            entry
                .operations
                .values()
                .any(|operation| operation.oauth_upstream.is_some())
        })
    }

    /// Return standalone MCP metadata for a uniquely named operation the native
    /// dispatcher can represent. Metadata is populated only by
    /// [`Self::load_mcp`]; missing/ambiguous descriptors stay unexposed.
    #[cfg(feature = "mcp-registry")]
    #[must_use]
    pub fn mcp_tool(
        &self,
        label: &str,
        id: &str,
    ) -> Option<(&str, &serde_json::Map<String, serde_json::Value>)> {
        let descriptor = self.inner.get(label)?.mcp.get(id)?.as_ref()?;
        Some((&descriptor.description, &descriptor.input))
    }

    /// Validate and return the tool metadata and read-only hint used by the
    /// standalone server. The complete selection must pass
    /// [`validate_mcp_operation_ids`] before the registry is loaded.
    ///
    /// # Errors
    /// Rejects missing or ambiguous operations, unsupported projections, and
    /// mutating operations without explicit opt-in.
    #[cfg(feature = "mcp-registry")]
    pub fn selected_mcp_tool(
        &self,
        label: &str,
        id: &str,
        allow_write: bool,
    ) -> Result<McpToolSelection<'_>, McpSelectionError> {
        let entry = self.inner.get(label);
        let method = entry
            .and_then(|entry| entry.operations.get(id))
            .map(|op| &op.method);
        let descriptor = entry
            .and_then(|entry| entry.mcp.get(id))
            .and_then(Option::as_ref);
        require_mcp_tool(id, method, descriptor, allow_write)
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

async fn load_one_spec(
    spec: OpenApiSpecConfig,
    project_mcp: bool,
) -> Result<(SpecEntry, Vec<String>), OpenApiError> {
    let base_url = crate::ssrf::validate_base_url(&spec)?;
    let spec_json = fetch_spec_json(&spec.spec_source, &spec.label).await?;
    #[cfg(feature = "mcp-registry")]
    let descriptors = if project_mcp {
        crate::convert::convert_mcp_spec(
            &spec.label,
            &spec_json,
            &spec.allowed_operations,
            MAX_OPERATIONS_PER_SPEC,
        )?
    } else {
        crate::convert::convert_spec(&spec.label, &spec_json, &spec.allowed_operations)?
    };
    #[cfg(not(feature = "mcp-registry"))]
    let descriptors = {
        let _ = project_mcp;
        crate::convert::convert_spec(&spec.label, &spec_json, &spec.allowed_operations)?
    };
    let converted = descriptors.len();
    let mut operations = HashMap::new();
    #[cfg(feature = "mcp-registry")]
    let mcp = if project_mcp {
        mcp_descriptors(&descriptors)
    } else {
        crate::mcp_schema::Tools::new()
    };
    for d in descriptors.into_iter().take(MAX_OPERATIONS_PER_SPEC) {
        operations.insert(
            d.operation_id.clone(),
            OperationHandle {
                operation_id: d.operation_id,
                method: d.method,
                path_template: d.path_template,
                base_url: base_url.clone(),
                credential: spec.credential.clone(),
                oauth_upstream: spec.oauth_upstream.clone(),
            },
        );
    }
    let mut warnings = Vec::new();
    if converted > MAX_OPERATIONS_PER_SPEC {
        warnings.push(format!(
            "OpenAPI spec `{}` exposes {converted} allowed operations but only {MAX_OPERATIONS_PER_SPEC} can be loaded; narrow allowed_operations",
            spec.label
        ));
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
        warnings.push(format!(
            "OpenAPI spec `{}` loaded but no operations matched allowed_operations; review the operation IDs in that allowlist",
            spec.label
        ));
        tracing::warn!(
            service = "openapi",
            label = %spec.label,
            allowed = spec.allowed_operations.len(),
            kind = "empty_allowlist",
            "openapi spec loaded but no operations matched the allowlist"
        );
    }
    Ok((
        SpecEntry {
            operations,
            #[cfg(feature = "mcp-registry")]
            mcp,
        },
        warnings,
    ))
}

/// Operator-safe diagnostics for a rejected standalone MCP selection.
#[cfg(feature = "mcp-schema")]
#[derive(Debug, thiserror::Error)]
pub enum McpSelectionError {
    /// The requested number of tools is outside the supported range.
    #[error("select between 1 and {MAX_OPERATIONS_PER_SPEC} OpenAPI operation IDs")]
    OperationCount,
    /// A requested name cannot be exposed as an MCP tool.
    #[error(
        "each operation ID must be a valid MCP tool name (letters, digits, underscores, dots or hyphens)"
    )]
    InvalidName,
    /// The caller asked for the same operation more than once.
    #[error("operation IDs must be unique")]
    DuplicateSelection,
    /// The bounded local spec could not be read or parsed.
    #[error(transparent)]
    Document(#[from] OpenApiError),
    /// A requested operation does not occur in a successfully loaded spec.
    #[error("operation {0} is absent or the OpenAPI document could not be loaded")]
    MissingOperation(String),
    /// A requested operation cannot be represented unambiguously by MCP.
    #[error("operation {0} has duplicate IDs or unsupported input metadata")]
    DuplicateOrUnsupported(String),
    /// A requested operation would permit writes without operator opt-in.
    #[error("operation {0} is not read-only; review it and pass --allow-write to expose it")]
    WriteNotAllowed(String),
}

/// Validate the bounded, exact names exposed by either MCP registration path.
///
/// # Errors
/// Rejects empty or oversized selections, invalid MCP names, and repeated IDs.
#[cfg(feature = "mcp-schema")]
pub fn validate_mcp_operation_ids(ids: &[String]) -> Result<(), McpSelectionError> {
    if !(1..=MAX_OPERATIONS_PER_SPEC).contains(&ids.len()) {
        return Err(McpSelectionError::OperationCount);
    }
    let mut seen = std::collections::HashSet::with_capacity(ids.len());
    for id in ids {
        validate_mcp_tool_name(id)?;
        if !seen.insert(id) {
            return Err(McpSelectionError::DuplicateSelection);
        }
    }
    Ok(())
}

#[cfg(feature = "mcp-schema")]
fn validate_mcp_tool_name(id: &str) -> Result<(), McpSelectionError> {
    if (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    {
        Ok(())
    } else {
        Err(McpSelectionError::InvalidName)
    }
}

/// A checked standalone MCP tool descriptor and dispatch classification.
#[cfg(feature = "mcp-schema")]
pub struct McpToolSelection<'a> {
    /// Bounded operator-provided description (or generated method/path).
    pub description: &'a str,
    /// MCP projection over the native dispatcher's flat input shape.
    pub schema: &'a serde_json::Map<String, serde_json::Value>,
    /// Whether the underlying method is GET or HEAD.
    pub read_only: bool,
}

#[cfg(feature = "mcp-schema")]
fn require_mcp_tool<'a>(
    id: &str,
    method: Option<&reqwest::Method>,
    descriptor: Option<&'a crate::mcp_schema::McpDescriptor>,
    allow_write: bool,
) -> Result<McpToolSelection<'a>, McpSelectionError> {
    validate_mcp_tool_name(id)?;
    let method = method.ok_or_else(|| McpSelectionError::MissingOperation(id.into()))?;
    let descriptor =
        descriptor.ok_or_else(|| McpSelectionError::DuplicateOrUnsupported(id.into()))?;
    let read_only = matches!(method.as_str(), "GET" | "HEAD");
    if !read_only && !allow_write {
        return Err(McpSelectionError::WriteNotAllowed(id.into()));
    }
    Ok(McpToolSelection {
        description: &descriptor.description,
        schema: &descriptor.input,
        read_only,
    })
}

#[cfg(feature = "mcp-schema")]
fn mcp_descriptors(
    descriptors: &[crate::convert::OperationDescriptor],
) -> crate::mcp_schema::Tools {
    use std::collections::hash_map::Entry;

    let mut mcp = crate::mcp_schema::Tools::new();
    // Conversion emits only the selected IDs; duplicates outside the selection
    // do not affect it. A selected duplicate is never exposed.
    for descriptor in descriptors.iter().take(MAX_OPERATIONS_PER_SPEC) {
        match mcp.entry(descriptor.operation_id.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(descriptor.mcp.clone());
            }
            Entry::Occupied(mut entry) => {
                entry.insert(None);
            }
        }
    }
    // A later duplicate still invalidates a retained MCP tool, without
    // projecting or allocating a schema for the beyond-cap descriptor.
    for descriptor in descriptors.iter().skip(MAX_OPERATIONS_PER_SPEC) {
        if let Some(previous) = mcp.get_mut(&descriptor.operation_id) {
            *previous = None;
        }
    }
    mcp
}

/// Preflight selected tools in a capped local JSON document. This does no DNS,
/// outbound HTTP, or process launch. Call from a blocking worker. Standalone
/// startup still validates the document again before advertising tools.
///
/// # Errors
/// Rejects invalid documents or selections and writes without opt-in.
#[cfg(feature = "mcp-schema")]
pub fn preflight_mcp_path(
    path: &Path,
    ids: &[String],
    allow_write: bool,
) -> Result<(), McpSelectionError> {
    const LABEL: &str = "guided registration";
    validate_mcp_operation_ids(ids)?;
    let json = read_path_capped_sync(path, MAX_SPEC_BYTES, LABEL)?;
    let operations = crate::convert::convert_mcp_spec(LABEL, &json, ids, MAX_OPERATIONS_PER_SPEC)?;
    let descriptors = mcp_descriptors(&operations);
    let methods: HashMap<_, _> = operations
        .iter()
        .map(|op| (op.operation_id.as_str(), &op.method))
        .collect();
    for id in ids {
        require_mcp_tool(
            id,
            methods.get(id.as_str()).copied(),
            descriptors.get(id).and_then(Option::as_ref),
            allow_write,
        )?;
    }
    Ok(())
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

async fn read_path_capped(path: &Path, cap: usize, label: &str) -> Result<String, OpenApiError> {
    let path = path.to_owned();
    let worker_label = label.to_owned();
    tokio::task::spawn_blocking(move || read_path_capped_sync(&path, cap, &worker_label))
        .await
        .map_err(|_| OpenApiError::SpecParse {
            label: label.to_owned(),
        })?
}

fn read_path_capped_sync(path: &Path, cap: usize, label: &str) -> Result<String, OpenApiError> {
    use std::io::Read;
    let parse_error = || OpenApiError::SpecParse {
        label: label.to_string(),
    };
    // Refuse special files before opening; NONBLOCK also closes the FIFO
    // replacement race on Unix between metadata and open.
    if !std::fs::metadata(path)
        .map_err(|_| parse_error())?
        .is_file()
    {
        return Err(parse_error());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
    }
    let file = options.open(path).map_err(|_| parse_error())?;
    if !file.metadata().map_err(|_| parse_error())?.is_file() {
        return Err(parse_error());
    }
    let mut bytes = Vec::new();
    file.take(cap as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| parse_error())?;
    if bytes.len() > cap {
        return Err(OpenApiError::SpecTooLarge {
            label: label.to_string(),
        });
    }
    String::from_utf8(bytes).map_err(|_| parse_error())
}

#[cfg(test)]
mod tests {

    #[tokio::test]
    async fn local_spec_read_enforces_exact_byte_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spec.json");
        std::fs::write(&path, b"1234").unwrap();
        assert_eq!(read_path_capped(&path, 4, "spec").await.unwrap(), "1234");
        std::fs::write(&path, b"12345").unwrap();
        assert!(matches!(
            read_path_capped(&path, 4, "spec").await,
            Err(OpenApiError::SpecTooLarge { .. })
        ));
        assert!(read_path_capped(dir.path(), 4, "spec").await.is_err());
    }

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
            oauth_upstream: None,
        }
    }

    fn bad_spec(label: &str, base: &str) -> OpenApiSpecConfig {
        OpenApiSpecConfig {
            label: label.into(),
            spec_source: SpecSource::Path(std::env::temp_dir().join("nonexistent.json")),
            base_url: base.parse().unwrap(),
            allowed_operations: vec![],
            credential: None,
            oauth_upstream: None,
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
            reg.warnings()
                .iter()
                .any(|warning| warning.contains("badlabel"))
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "concurrent + bounded"
        );
    }

    #[cfg(feature = "mcp-registry")]
    #[tokio::test]
    async fn mcp_projection_is_explicit_and_selected_duplicates_beyond_cap_are_refused() {
        let spec = good_fixture_spec("mcp-profile");
        let config = OpenApiProviderConfig { specs: vec![spec] };
        let normal = OpenApiRegistry::load(config.clone(), Duration::from_secs(2)).await;
        assert!(normal.operation("mcp-profile", "getUser").is_ok());
        assert!(normal.mcp_tool("mcp-profile", "getUser").is_none());
        let standalone = OpenApiRegistry::load_mcp(config, Duration::from_secs(2)).await;
        assert!(standalone.mcp_tool("mcp-profile", "getUser").is_some());

        let paths = (0..=MAX_OPERATIONS_PER_SPEC)
            .map(|i| {
                (
                    format!("/item-{i:04}"),
                    serde_json::json!({"get": {"operationId": "duplicateItem"}}),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        let document = serde_json::json!({"openapi": "3.0.0", "paths": paths}).to_string();
        let mut paths: serde_json::Value = serde_json::from_str(&document).unwrap();
        for i in 1..MAX_OPERATIONS_PER_SPEC {
            paths["paths"][format!("/item-{i:04}")]["get"]["operationId"] =
                serde_json::Value::String(format!("op-{i}"));
        }
        let ids: Vec<_> = std::iter::once("duplicateItem".into())
            .chain((1..MAX_OPERATIONS_PER_SPEC).map(|i| format!("op-{i}")))
            .collect();
        let ops = crate::convert::convert_mcp_spec(
            "duplicate-profile",
            &paths.to_string(),
            &ids,
            MAX_OPERATIONS_PER_SPEC,
        )
        .unwrap();
        assert_eq!(ops.len(), MAX_OPERATIONS_PER_SPEC + 1);
        assert!(ops[0].mcp.is_some());
        assert!(ops[MAX_OPERATIONS_PER_SPEC].mcp.is_none());
        let descriptors = mcp_descriptors(&ops);
        assert!(descriptors["duplicateItem"].is_none());
        assert!(descriptors["op-1"].is_some());
    }

    #[tokio::test]
    async fn good_spec_exposes_allowed_operation() {
        let cfg = OpenApiProviderConfig {
            specs: vec![good_fixture_spec("vendor")],
        };
        let reg = OpenApiRegistry::load(cfg, Duration::from_secs(2)).await;
        assert_eq!(reg.warnings(), Vec::<String>::new());
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
