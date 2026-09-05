//! Typed client and Code Mode projection for Unraid Core's private provider.
//!
//! This is not an MCP client. It consumes the bounded Core provider protocol
//! over a Unix socket and projects actor-filtered operations into Labby's
//! existing Code Mode host catalog under the reserved `unraid` namespace.

#[cfg(unix)]
use std::time::Duration;
use std::{collections::HashSet, path::Path};

use futures::StreamExt;
use labby_codemode::{
    CodeModeToolSafety, ToolDescriptor, ToolScope, ToolsRender, discovery_entry_visible,
};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};

const PROVIDER_URL: &str = "http://unraid-core.local/v1/provider";
const PROVIDER_PROTOCOL: &str = "1.0";
const REQUEST_BYTES_MAX: usize = 1024 * 1024;
const DISCOVERY_RESPONSE_BYTES_MAX: usize = 1024 * 1024;
const EXECUTE_RESPONSE_BYTES_MAX: usize = 2 * 1024 * 1024;
const MAX_CATALOG_PAGES: usize = 20;
const PAGE_SIZE: usize = 50;
const RESPONSE_HEADER_BYTES_MAX: usize = 32 * 1024;
const RESPONSE_HEADER_COUNT_MAX: usize = 100;
#[cfg(unix)]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum CoreProviderError {
    #[error("Core provider configuration is invalid")]
    Configuration,
    #[error("Core provider request is too large")]
    RequestTooLarge,
    #[error("Core provider response is too large")]
    ResponseTooLarge,
    #[error("Core provider is unavailable")]
    Unavailable,
    #[error("Core provider denied the request")]
    Denied,
    #[error("Core provider returned an incompatible response")]
    Incompatible,
}

#[derive(Clone)]
pub struct CoreProviderClient {
    client: reqwest::Client,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CoreOperationDescriptor {
    pub id: String,
    pub helper: String,
    pub kind: String,
    pub field: String,
    #[serde(default)]
    pub path: Vec<String>,
    pub owner: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub arguments: Vec<CoreArgument>,
    pub result_type: String,
    pub deprecation: Option<String>,
    pub policy: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CoreArgument {
    pub name: String,
    #[serde(rename = "type")]
    pub type_ref: String,
    pub default: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct CoreCodeModeTool {
    pub operation_id: String,
    pub schema_version: String,
    pub descriptor: ToolDescriptor,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    results: Vec<CoreOperationDescriptor>,
    next_cursor: Option<String>,
    schema_version: String,
}

#[derive(Debug, Deserialize)]
struct ProviderHealth {
    status: String,
    provider_protocol: String,
}

#[derive(Debug, Serialize)]
struct ProviderRequest<'a> {
    provider_protocol: &'static str,
    op: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    query: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    page_size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    variables: Option<&'a Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_schema_version: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<&'a str>,
}

impl CoreProviderClient {
    #[cfg(unix)]
    pub fn new(socket_path: impl AsRef<Path>) -> Result<Self, CoreProviderError> {
        let socket_path = socket_path.as_ref();
        if !socket_path.is_absolute() || socket_path.as_os_str().is_empty() {
            return Err(CoreProviderError::Configuration);
        }
        let client = reqwest::Client::builder()
            .unix_socket(socket_path)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| CoreProviderError::Configuration)?;
        Ok(Self { client })
    }

    #[cfg(not(unix))]
    pub fn new(_socket_path: impl AsRef<Path>) -> Result<Self, CoreProviderError> {
        Err(CoreProviderError::Configuration)
    }

    /// Fetch every actor-visible operation and project it into Code Mode.
    pub async fn code_mode_catalog(
        &self,
        delegated_assertion: &str,
    ) -> Result<Vec<CoreCodeModeTool>, CoreProviderError> {
        let mut cursor = None;
        let mut expected_version: Option<String> = None;
        let mut tools = Vec::new();
        let mut operation_ids = HashSet::new();
        let mut helpers = HashSet::new();
        let mut cursors = HashSet::new();

        for _page in 0..MAX_CATALOG_PAGES {
            let response: SearchResponse = self
                .request(
                    delegated_assertion,
                    &ProviderRequest {
                        provider_protocol: PROVIDER_PROTOCOL,
                        op: "search",
                        query: Some(""),
                        page_size: Some(PAGE_SIZE),
                        cursor: cursor.as_deref(),
                        operation_id: None,
                        variables: None,
                        expected_schema_version: None,
                        request_id: None,
                    },
                    DISCOVERY_RESPONSE_BYTES_MAX,
                )
                .await?;

            if !valid_schema_version(&response.schema_version)
                || response.results.len() > PAGE_SIZE
                || expected_version
                    .as_ref()
                    .is_some_and(|version| version != &response.schema_version)
            {
                return Err(CoreProviderError::Incompatible);
            }
            expected_version = Some(response.schema_version.clone());
            for operation in response.results {
                if !operation_ids.insert(operation.id.clone())
                    || !helpers.insert(operation.helper.clone())
                {
                    return Err(CoreProviderError::Incompatible);
                }
                tools.push(project_operation(
                    operation,
                    response.schema_version.clone(),
                )?);
            }

            match response.next_cursor {
                Some(next) if valid_cursor(&next) && cursors.insert(next.clone()) => {
                    cursor = Some(next);
                }
                Some(_) => return Err(CoreProviderError::Incompatible),
                None => return Ok(tools),
            }
        }
        Err(CoreProviderError::Incompatible)
    }

    pub async fn health(&self) -> Result<(), CoreProviderError> {
        let response: ProviderHealth = self
            .request_without_assertion(
                &ProviderRequest {
                    provider_protocol: PROVIDER_PROTOCOL,
                    op: "health",
                    query: None,
                    page_size: None,
                    cursor: None,
                    operation_id: None,
                    variables: None,
                    expected_schema_version: None,
                    request_id: None,
                },
                DISCOVERY_RESPONSE_BYTES_MAX,
            )
            .await?;

        if response.status == "ready" && response.provider_protocol == PROVIDER_PROTOCOL {
            Ok(())
        } else {
            Err(CoreProviderError::Incompatible)
        }
    }

    pub async fn execute(
        &self,
        delegated_assertion: &str,
        request_id: &str,
        operation_id: &str,
        variables: &Value,
        expected_schema_version: &str,
    ) -> Result<Value, CoreProviderError> {
        if !valid_request_id(request_id) {
            return Err(CoreProviderError::Configuration);
        }
        let response: Value = self
            .request(
                delegated_assertion,
                &ProviderRequest {
                    provider_protocol: PROVIDER_PROTOCOL,
                    op: "execute",
                    query: None,
                    page_size: None,
                    cursor: None,
                    operation_id: Some(operation_id),
                    variables: Some(variables),
                    expected_schema_version: Some(expected_schema_version),
                    request_id: Some(request_id),
                },
                EXECUTE_RESPONSE_BYTES_MAX,
            )
            .await?;
        if valid_execute_outcome(&response) {
            Ok(response)
        } else {
            Err(CoreProviderError::Incompatible)
        }
    }

    pub async fn cancel(
        &self,
        delegated_assertion: &str,
        request_id: &str,
    ) -> Result<Value, CoreProviderError> {
        if !valid_request_id(request_id) {
            return Err(CoreProviderError::Configuration);
        }
        self.request(
            delegated_assertion,
            &ProviderRequest {
                provider_protocol: PROVIDER_PROTOCOL,
                op: "cancel",
                query: None,
                page_size: None,
                cursor: None,
                operation_id: None,
                variables: None,
                expected_schema_version: None,
                request_id: Some(request_id),
            },
            DISCOVERY_RESPONSE_BYTES_MAX,
        )
        .await
    }

    async fn request<T: DeserializeOwned>(
        &self,
        delegated_assertion: &str,
        request: &ProviderRequest<'_>,
        response_limit: usize,
    ) -> Result<T, CoreProviderError> {
        if delegated_assertion.is_empty() || delegated_assertion.len() > 16 * 1024 {
            return Err(CoreProviderError::Configuration);
        }
        self.request_inner(Some(delegated_assertion), request, response_limit)
            .await
    }

    async fn request_without_assertion<T: DeserializeOwned>(
        &self,
        request: &ProviderRequest<'_>,
        response_limit: usize,
    ) -> Result<T, CoreProviderError> {
        self.request_inner(None, request, response_limit).await
    }

    async fn request_inner<T: DeserializeOwned>(
        &self,
        delegated_assertion: Option<&str>,
        request: &ProviderRequest<'_>,
        response_limit: usize,
    ) -> Result<T, CoreProviderError> {
        let body = serde_json::to_vec(request).map_err(|_| CoreProviderError::Configuration)?;
        if body.len() > REQUEST_BYTES_MAX {
            return Err(CoreProviderError::RequestTooLarge);
        }

        let mut request = self
            .client
            .post(PROVIDER_URL)
            .header(CONTENT_TYPE, "application/json")
            .body(body);
        if let Some(delegated_assertion) = delegated_assertion {
            request = request.header(AUTHORIZATION, format!("Bearer {delegated_assertion}"));
        }
        let response = request
            .send()
            .await
            .map_err(|_| CoreProviderError::Unavailable)?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            || response.status() == reqwest::StatusCode::FORBIDDEN
        {
            return Err(CoreProviderError::Denied);
        }
        if !response.status().is_success() {
            return Err(CoreProviderError::Unavailable);
        }
        let headers = response.headers();
        let header_bytes = headers.iter().try_fold(0_usize, |total, (name, value)| {
            total
                .checked_add(name.as_str().len())?
                .checked_add(value.as_bytes().len())
        });
        if headers.len() > RESPONSE_HEADER_COUNT_MAX
            || header_bytes.is_none_or(|bytes| bytes > RESPONSE_HEADER_BYTES_MAX)
        {
            return Err(CoreProviderError::ResponseTooLarge);
        }
        if response
            .content_length()
            .is_some_and(|length| length > response_limit as u64)
        {
            return Err(CoreProviderError::ResponseTooLarge);
        }

        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| CoreProviderError::Unavailable)?;
            if bytes.len().saturating_add(chunk.len()) > response_limit {
                return Err(CoreProviderError::ResponseTooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| CoreProviderError::Incompatible)
    }
}

fn project_operation(
    operation: CoreOperationDescriptor,
    schema_version: String,
) -> Result<CoreCodeModeTool, CoreProviderError> {
    if !valid_operation(&operation) {
        return Err(CoreProviderError::Incompatible);
    }
    let safety = operation_safety(&operation)?;
    let descriptor = ToolDescriptor::tool_with_safety(
        "unraid",
        &operation.helper,
        &operation.summary,
        Some(arguments_schema(&operation.arguments)),
        Some(json!({})),
        safety,
    );
    Ok(CoreCodeModeTool {
        operation_id: operation.id,
        schema_version,
        descriptor,
    })
}

fn valid_schema_version(version: &str) -> bool {
    version.len() <= 256 && version.starts_with("sha256:")
}

fn valid_request_id(request_id: &str) -> bool {
    !request_id.is_empty()
        && request_id.len() <= 256
        && request_id.trim() == request_id
        && !request_id.chars().any(char::is_control)
}

fn valid_cursor(cursor: &str) -> bool {
    !cursor.is_empty()
        && cursor.len() <= 4096
        && cursor.trim() == cursor
        && !cursor.chars().any(char::is_control)
}

fn valid_execute_outcome(response: &Value) -> bool {
    response
        .as_object()
        .and_then(|object| object.get("outcome"))
        .and_then(Value::as_str)
        .is_some_and(|outcome| {
            matches!(
                outcome,
                "complete"
                    | "partial"
                    | "denied"
                    | "approval_required"
                    | "schema_stale"
                    | "cancelled_before_attempt"
                    | "cancelled_after_attempt"
                    | "outcome_unknown"
                    | "failed"
            )
        })
}

fn operation_safety(
    operation: &CoreOperationDescriptor,
) -> Result<Option<CodeModeToolSafety>, CoreProviderError> {
    let policy = operation
        .policy
        .as_ref()
        .and_then(Value::as_object)
        .ok_or(CoreProviderError::Incompatible)?;
    let read_only = policy
        .get("read_only")
        .and_then(Value::as_bool)
        .ok_or(CoreProviderError::Incompatible)?;
    let destructive = policy
        .get("destructive")
        .and_then(Value::as_bool)
        .ok_or(CoreProviderError::Incompatible)?;
    if (operation.kind == "query" && (!read_only || destructive))
        || (operation.kind == "mutation" && read_only)
    {
        return Err(CoreProviderError::Incompatible);
    }
    Ok(Some(CodeModeToolSafety {
        read_only: Some(read_only),
        destructive: Some(destructive),
    }))
}

fn valid_operation(operation: &CoreOperationDescriptor) -> bool {
    let valid_kind = matches!(operation.kind.as_str(), "query" | "mutation");
    let valid_helper = !operation.helper.is_empty()
        && operation.helper.len() <= 256
        && operation
            .helper
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
    let valid_id = !operation.id.is_empty()
        && operation.id.len() <= 512
        && operation.id.ends_with("@v1")
        && !operation.id.bytes().any(|byte| byte.is_ascii_control());
    valid_kind && valid_helper && valid_id
}

/// Merge actor-visible Core tools into the existing host render while applying
/// the same fine-grained scope filter used for ordinary upstream tools.
pub fn merge_tools_render(
    base: ToolsRender,
    core_tools: Vec<CoreCodeModeTool>,
    scope: &ToolScope,
) -> Result<ToolsRender, CoreProviderError> {
    let mut entries = base.entries.iter().cloned().collect::<Vec<_>>();
    entries.extend(
        core_tools
            .into_iter()
            .map(|tool| tool.descriptor)
            .filter(|descriptor| discovery_entry_visible(descriptor, scope)),
    );
    entries.sort_by(|left, right| left.id.cmp(&right.id));
    let catalog_json =
        serde_json::to_string(&entries).map_err(|_| CoreProviderError::Incompatible)?;
    let fingerprint = digest_strings(
        std::iter::once(base.fingerprint.as_str())
            .chain(entries.iter().map(|entry| entry.id.as_str())),
    );
    let embedding_fingerprint = digest_strings(
        entries
            .iter()
            .flat_map(|entry| [entry.id.as_str(), entry.description.as_str()]),
    );
    Ok(ToolsRender {
        fingerprint,
        embedding_fingerprint,
        entries: entries.into(),
        serialized_size: catalog_json.len(),
        catalog_json: catalog_json.into(),
    })
}

fn digest_strings<'a>(values: impl IntoIterator<Item = &'a str>) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    for value in values {
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value.as_bytes());
    }
    hex::encode(digest.finalize())
}

fn arguments_schema(arguments: &[CoreArgument]) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for argument in arguments {
        properties.insert(
            argument.name.clone(),
            graphql_type_schema(&argument.type_ref),
        );
        if argument.type_ref.ends_with('!') && argument.default.is_none() {
            required.push(Value::String(argument.name.clone()));
        }
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn graphql_type_schema(type_ref: &str) -> Value {
    let required = type_ref.ends_with('!');
    let base = type_ref.trim_end_matches('!');
    let mut schema = if base.starts_with('[') && base.ends_with(']') {
        json!({"type": "array", "items": graphql_type_schema(&base[1..base.len() - 1])})
    } else {
        match base {
            "Boolean" => json!({"type": "boolean"}),
            "Int" => json!({"type": "integer"}),
            "Float" => json!({"type": "number"}),
            "String" | "ID" | "PrefixedID" => json!({"type": "string"}),
            _ => json!({}),
        }
    };
    if !required && let Some(object) = schema.as_object_mut() {
        object.insert("nullable".to_string(), Value::Bool(true));
    }
    schema
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    #[test]
    fn shared_provider_fixture_matches_the_client_contract() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../docs/contracts/core-provider-protocol-v1.json"
        ))
        .unwrap();

        assert_eq!(fixture["provider_protocol"]["version"], PROVIDER_PROTOCOL);
        assert_eq!(fixture["operations"]["search"]["page_default"], 20);
        assert_eq!(fixture["operations"]["search"]["page_max"], PAGE_SIZE);
        assert_eq!(fixture["framing"]["execute_concurrency_limit"], 16);
        assert_eq!(fixture["actor_assertion_profile"]["ttl_seconds_max"], 60);
        for fixture in fixture["fixtures"].as_array().unwrap() {
            for field in ["schema_version", "expected_schema_version"] {
                if let Some(version) = fixture["request"]
                    .get(field)
                    .or_else(|| fixture["response"].get(field))
                    .and_then(Value::as_str)
                    && version != "old"
                {
                    assert!(valid_schema_version(version));
                }
            }
        }
    }

    #[test]
    fn projects_core_operation_as_unraid_code_mode_tool() {
        let operation = CoreOperationDescriptor {
            id: "core.graphql.query.log_file@v1".to_string(),
            helper: "core_query_log_file".to_string(),
            kind: "query".to_string(),
            field: "log_file".to_string(),
            path: vec!["log_file".to_string()],
            owner: "core".to_string(),
            summary: "Read a bounded log file".to_string(),
            arguments: vec![CoreArgument {
                name: "path".to_string(),
                type_ref: "String!".to_string(),
                default: None,
            }],
            result_type: "LogFileContent!".to_string(),
            deprecation: None,
            policy: Some(json!({"read_only": true, "destructive": false})),
        };

        let projected = project_operation(operation, "sha256:test".to_string()).unwrap();
        assert_eq!(projected.descriptor.id, "unraid::core_query_log_file");
        assert_eq!(projected.operation_id, "core.graphql.query.log_file@v1");
        assert_eq!(projected.schema_version, "sha256:test");
        assert_eq!(
            projected.descriptor.safety,
            Some(CodeModeToolSafety {
                read_only: Some(true),
                destructive: Some(false)
            })
        );
        assert_eq!(
            projected.descriptor.schema.as_ref().unwrap()["required"],
            json!(["path"])
        );
    }

    #[test]
    fn mutation_safety_is_projected_from_explicit_policy() {
        let operation = CoreOperationDescriptor {
            id: "core.graphql.mutation.docker.start@v1".to_string(),
            helper: "core_mutation_docker_start".to_string(),
            kind: "mutation".to_string(),
            field: "docker.start".to_string(),
            path: vec!["docker".to_string(), "start".to_string()],
            owner: "core".to_string(),
            summary: String::new(),
            arguments: Vec::new(),
            result_type: "DockerContainer!".to_string(),
            deprecation: None,
            policy: Some(json!({"read_only": false, "destructive": false})),
        };
        let projected = project_operation(operation, "sha256:test".to_string()).unwrap();
        assert_eq!(
            projected.descriptor.safety,
            Some(CodeModeToolSafety {
                read_only: Some(false),
                destructive: Some(false)
            })
        );
    }

    #[test]
    fn rejects_operations_without_explicit_boolean_safety_policy() {
        let operation = CoreOperationDescriptor {
            id: "core.graphql.mutation.docker.start@v1".to_string(),
            helper: "core_mutation_docker_start".to_string(),
            kind: "mutation".to_string(),
            field: "docker.start".to_string(),
            path: vec!["docker".to_string(), "start".to_string()],
            owner: "core".to_string(),
            summary: String::new(),
            arguments: Vec::new(),
            result_type: "DockerContainer!".to_string(),
            deprecation: None,
            policy: Some(json!({"approval": "interactive"})),
        };

        assert!(matches!(
            project_operation(operation, "sha256:test".to_string()),
            Err(CoreProviderError::Incompatible)
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn client_uses_the_unix_socket_and_versioned_bearer_request() {
        drop(rustls::crypto::ring::default_provider().install_default());
        let directory = tempfile::tempdir().unwrap();
        let socket_path = directory.path().join("provider.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            serve_once(
                listener,
                json!({
                    "results": [{
                        "id": "core.graphql.query.health@v1",
                        "helper": "core_query_health",
                        "kind": "query",
                        "field": "health",
                        "path": ["health"],
                        "owner": "core",
                        "summary": "Core health",
                        "arguments": [],
                        "result_type": "Health!",
                        "deprecation": null,
                        "policy": {"read_only": true, "destructive": false}
                    }],
                    "next_cursor": null,
                    "schema_version": "sha256:test"
                }),
            )
            .await
        });

        let client = CoreProviderClient::new(&socket_path).unwrap();
        let catalog = client
            .code_mode_catalog("delegated-assertion")
            .await
            .unwrap();
        let request = server.await.unwrap();

        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].descriptor.id, "unraid::core_query_health");
        assert!(request.contains("authorization: Bearer delegated-assertion"));
        assert!(request.contains("\"provider_protocol\":\"1.0\""));
        assert!(request.contains("\"op\":\"search\""));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn catalog_rejects_duplicate_operation_ids_or_helpers() {
        drop(rustls::crypto::ring::default_provider().install_default());
        let directory = tempfile::tempdir().unwrap();
        let socket_path = directory.path().join("provider.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
        let operation = json!({
            "id": "core.graphql.query.health@v1",
            "helper": "core_query_health",
            "kind": "query",
            "field": "health",
            "path": ["health"],
            "owner": "core",
            "summary": "Core health",
            "arguments": [],
            "result_type": "Health!",
            "deprecation": null,
            "policy": {"read_only": true, "destructive": false}
        });
        let server = tokio::spawn(async move {
            serve_once(
                listener,
                json!({
                    "results": [operation.clone(), operation],
                    "next_cursor": null,
                    "schema_version": "sha256:test"
                }),
            )
            .await
        });

        let client = CoreProviderClient::new(&socket_path).unwrap();
        let result = client.code_mode_catalog("delegated-assertion").await;
        server.await.unwrap();

        assert!(matches!(result, Err(CoreProviderError::Incompatible)));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_sends_an_independent_provider_request_id() {
        drop(rustls::crypto::ring::default_provider().install_default());
        let directory = tempfile::tempdir().unwrap();
        let socket_path = directory.path().join("provider.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            serve_once(
                listener,
                json!({"outcome": "complete", "request_id": "provider-call-1"}),
            )
            .await
        });

        let client = CoreProviderClient::new(&socket_path).unwrap();
        let result = client
            .execute(
                "delegated-assertion",
                "provider-call-1",
                "core.graphql.query.health@v1",
                &json!({}),
                "sha256:test",
            )
            .await
            .unwrap();
        let request = server.await.unwrap();

        assert_eq!(result["request_id"], "provider-call-1");
        assert!(request.contains("\"request_id\":\"provider-call-1\""));
        assert!(request.contains("\"op\":\"execute\""));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_rejects_an_outcome_outside_the_closed_union() {
        drop(rustls::crypto::ring::default_provider().install_default());
        let directory = tempfile::tempdir().unwrap();
        let socket_path = directory.path().join("provider.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            serve_once(
                listener,
                json!({"outcome": "try_again", "request_id": "provider-call-2"}),
            )
            .await
        });

        let client = CoreProviderClient::new(&socket_path).unwrap();
        let result = client
            .execute(
                "delegated-assertion",
                "provider-call-2",
                "core.graphql.query.health@v1",
                &json!({}),
                "sha256:test",
            )
            .await;
        server.await.unwrap();

        assert!(matches!(result, Err(CoreProviderError::Incompatible)));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn health_is_transport_only_and_validates_the_protocol() {
        drop(rustls::crypto::ring::default_provider().install_default());
        let directory = tempfile::tempdir().unwrap();
        let socket_path = directory.path().join("provider.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            serve_once(
                listener,
                json!({"status": "ready", "provider_protocol": "1.0"}),
            )
            .await
        });

        let client = CoreProviderClient::new(&socket_path).unwrap();
        client.health().await.unwrap();
        let request = server.await.unwrap();

        assert!(request.contains("\"op\":\"health\""));
        assert!(!request.contains("authorization:"));
    }

    #[cfg(unix)]
    async fn serve_once(listener: tokio::net::UnixListener, response: Value) -> String {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let expected_length = loop {
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0);
            request.extend_from_slice(&buffer[..read]);
            if let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("content-length: ")
                            .and_then(|value| value.parse::<usize>().ok())
                    })
                    .unwrap();
                break header_end + 4 + content_length;
            }
        };
        while request.len() < expected_length {
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0);
            request.extend_from_slice(&buffer[..read]);
        }

        let response = serde_json::to_vec(&response).unwrap();
        let headers = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            response.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&response).await.unwrap();
        stream.shutdown().await.unwrap();
        String::from_utf8(request).unwrap()
    }
}
