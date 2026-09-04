//! Typed client for Labby's remote Artifact control-plane authority.
//!
//! The provider operation names are deliberately sealed in this module. Product
//! dispatchers select from the sealed `Operation` enum instead of forwarding arbitrary remote
//! operation strings supplied by a caller.

use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::core::{ApiError, HttpClient};

const MAX_CONTROL_PLANE_RESPONSE_BYTES: usize = 1024 * 1024;

/// Curated remote operations needed by Labby's public control-plane actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    ArtifactsList,
    ArtifactsGet,
    ArtifactsSearch,
    CandidatesList,
    CandidatesIntake,
    ArtifactsFollow,
    ArtifactsFork,
    ArtifactsSetPublication,
    ArtifactsSetLicense,
    SearchSkillsSh,
    SearchArd,
    SearchMarketplace,
    McpRegistryList,
    AcpRegistryList,
    AuthorityStatus,
    SourcesList,
    SourcesConfigure,
    SourcesDelete,
    SourcesRefresh,
    JobsStart,
    JobsList,
    JobsGet,
    JobsCancel,
    JobsRetry,
    UploadsCreate,
    UploadsGet,
    UploadsDelete,
    BundlesList,
    BundlesGet,
    BundlesCreate,
    BundlesAddArtifact,
    BundlesRemoveArtifact,
    BundlesSetVisibility,
    BundlesPublish,
    BundlesDelete,
}

impl Operation {
    const fn provider_name(self) -> &'static str {
        match self {
            Self::ArtifactsList => "depot.artifacts.list",
            Self::ArtifactsGet => "depot.artifacts.get",
            Self::ArtifactsSearch => "depot.skills.search",
            Self::CandidatesList => "depot.artifacts.list_candidates",
            Self::CandidatesIntake => "depot.artifacts.intake_candidate",
            Self::ArtifactsFollow => "depot.artifacts.follow",
            Self::ArtifactsFork => "depot.artifacts.fork",
            Self::ArtifactsSetPublication => "depot.artifacts.set_publication",
            Self::ArtifactsSetLicense => "depot.artifacts.set_license",
            Self::SearchSkillsSh => "depot.skills.search_skills_sh",
            Self::SearchArd => "depot.skills.search_ard",
            Self::SearchMarketplace => "depot.skills.search_marketplace",
            Self::McpRegistryList => "depot.mcp_registry.list",
            Self::AcpRegistryList => "depot.acp_registry.list",
            Self::AuthorityStatus => "depot.system.status",
            Self::SourcesList => "depot.sources.list",
            Self::SourcesConfigure => "depot.sources.configure",
            Self::SourcesDelete => "depot.sources.delete",
            Self::SourcesRefresh => "depot.sources.refresh",
            Self::JobsStart => "depot.ingest.start",
            Self::JobsList => "depot.ingest.list",
            Self::JobsGet => "depot.ingest.get",
            Self::JobsCancel => "depot.ingest.cancel",
            Self::JobsRetry => "depot.ingest.retry",
            Self::UploadsCreate => "depot.uploads.create",
            Self::UploadsGet => "depot.uploads.get",
            Self::UploadsDelete => "depot.uploads.delete",
            Self::BundlesList => "depot.bundles.list",
            Self::BundlesGet => "depot.bundles.get",
            Self::BundlesCreate => "depot.bundles.create",
            Self::BundlesAddArtifact => "depot.bundles.add_skill",
            Self::BundlesRemoveArtifact => "depot.bundles.remove_skill",
            Self::BundlesSetVisibility => "depot.bundles.set_visibility",
            Self::BundlesPublish => "depot.bundles.publish",
            Self::BundlesDelete => "depot.bundles.delete",
        }
    }

    fn expected_input_schema(self) -> Value {
        let properties = match self {
            Self::ArtifactsList => json!({"cursor":{"type":"string"},"limit":{"type":"integer"}}),
            Self::ArtifactsGet => json!({"artifactId":{"type":"string"}}),
            Self::ArtifactsSearch | Self::SearchSkillsSh => {
                json!({"query":{"type":"string"},"limit":{"type":"integer"}})
            }
            Self::CandidatesList => json!({"cursor":{"type":"string"},"limit":{"type":"integer"}}),
            Self::CandidatesIntake => json!({"candidate":{"type":"object"}}),
            Self::ArtifactsFollow => {
                json!({"artifactId":{"type":"string"},"upstreamArtifactId":{"type":"string"},"upstreamRevisionId":{"type":"string"},"following":{"type":"boolean"}})
            }
            Self::ArtifactsFork => {
                json!({"sourceArtifactId":{"type":"string"},"revisionId":{"type":"string"},"namespace":{"type":"string"},"name":{"type":"string"},"following":{"type":"boolean"}})
            }
            Self::ArtifactsSetPublication => {
                json!({"artifactId":{"type":"string"},"state":{"type":"string"},"visibility":{"type":"string"},"distribution":{"type":"string"}})
            }
            Self::ArtifactsSetLicense => {
                json!({"artifactId":{"type":"string"},"declared":{},"detected":{"type":"array"},"notices":{"type":"array"},"redistribution":{"type":"string"},"reviewState":{"type":"string"},"takedownState":{"type":"string"},"evidenceAt":{"type":"string"},"metadata":{"type":"object"}})
            }
            Self::SearchArd => {
                json!({"registry":{"type":"string"},"query":{"type":"string"},"pageToken":{"type":"string"}})
            }
            Self::SearchMarketplace => {
                json!({"source":{"type":"string"},"ref":{"type":"string"},"only":{"type":"array"}})
            }
            Self::McpRegistryList => {
                json!({"query":{"type":"string"},"category":{"type":"string"},"tag":{"type":"string"},"version":{"type":"string"},"updatedSince":{"type":"string"},"includeDeleted":{"type":"boolean"},"cursor":{"type":"string"},"limit":{"type":"integer"}})
            }
            Self::AcpRegistryList
            | Self::AuthorityStatus
            | Self::SourcesList
            | Self::BundlesList => json!({}),
            Self::SourcesConfigure => {
                json!({"sourceId":{"type":"string"},"enabled":{"type":"boolean"},"intervalSeconds":{"type":"integer"}})
            }
            Self::SourcesDelete | Self::SourcesRefresh => json!({"sourceId":{"type":"string"}}),
            Self::JobsStart => {
                json!({"kind":{"type":"string"},"arguments":{"type":"object"},"idempotencyKey":{"type":"string"}})
            }
            Self::JobsList => json!({"limit":{"type":"integer"}}),
            Self::JobsGet | Self::JobsCancel | Self::JobsRetry => {
                json!({"jobId":{"type":"string"}})
            }
            Self::UploadsCreate => json!({"filename":{"type":"string"}}),
            Self::UploadsGet | Self::UploadsDelete => json!({"uploadId":{"type":"string"}}),
            Self::BundlesGet | Self::BundlesPublish | Self::BundlesDelete => {
                json!({"slug":{"type":"string"}})
            }
            Self::BundlesCreate => {
                json!({"slug":{"type":"string"},"description":{"type":"string"},"visibility":{"type":"string"}})
            }
            Self::BundlesAddArtifact | Self::BundlesRemoveArtifact => {
                json!({"slug":{"type":"string"},"namespace":{"type":"string"},"name":{"type":"string"}})
            }
            Self::BundlesSetVisibility => {
                json!({"slug":{"type":"string"},"visibility":{"type":"string"}})
            }
        };
        let required = match self {
            Self::ArtifactsGet
            | Self::ArtifactsFollow
            | Self::ArtifactsSetPublication
            | Self::ArtifactsSetLicense => json!(["artifactId"]),
            Self::ArtifactsSearch | Self::SearchSkillsSh => json!(["query"]),
            Self::CandidatesIntake => json!(["candidate"]),
            Self::ArtifactsFork => json!(["sourceArtifactId", "namespace", "name"]),
            Self::SearchArd => json!(["registry", "query"]),
            Self::SearchMarketplace => json!(["source"]),
            Self::SourcesConfigure | Self::SourcesDelete | Self::SourcesRefresh => {
                json!(["sourceId"])
            }
            Self::JobsStart => json!(["kind", "arguments"]),
            Self::JobsGet | Self::JobsCancel | Self::JobsRetry => json!(["jobId"]),
            Self::UploadsCreate => json!(["filename"]),
            Self::UploadsGet | Self::UploadsDelete => json!(["uploadId"]),
            Self::BundlesGet | Self::BundlesPublish | Self::BundlesDelete => json!(["slug"]),
            Self::BundlesCreate => json!(["slug"]),
            Self::BundlesSetVisibility => json!(["slug", "visibility"]),
            Self::BundlesAddArtifact | Self::BundlesRemoveArtifact => {
                json!(["slug", "namespace", "name"])
            }
            _ => json!([]),
        };
        json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false
        })
    }

    fn expected_schema_fingerprint(self) -> String {
        schema_fingerprint(&self.expected_input_schema())
    }
}

#[derive(Debug, Deserialize)]
struct OperationEnvelope {
    result: Value,
}

#[derive(Debug, Deserialize)]
struct OperationCatalog {
    operations: Vec<Value>,
}

const OPERATION_CONTRACT_VERSION: u64 = 1;

/// Remote authority client. Construction is pure; the product binary owns
/// endpoint validation, DNS pinning, and server-held credential resolution.
#[derive(Debug, Clone)]
pub struct ArtifactControlClient {
    http: HttpClient,
}

impl ArtifactControlClient {
    #[must_use]
    pub const fn new(http: HttpClient) -> Self {
        Self { http }
    }

    /// Execute one curated operation and unwrap the provider envelope.
    ///
    /// # Errors
    /// Returns the shared API error taxonomy for transport, authorization,
    /// upstream status, or malformed envelopes.
    pub async fn execute(&self, operation: Operation, params: &Value) -> Result<Value, ApiError> {
        let catalog: OperationCatalog = self
            .http
            .get_json_bounded("/api/operations", MAX_CONTROL_PLANE_RESPONSE_BYTES)
            .await?;
        let compatible = catalog.operations.iter().any(|definition| {
            let input_schema = definition.get("inputSchema");
            let declared_fingerprint = definition.get("schemaFingerprint").and_then(Value::as_str);
            let expected_fingerprint = operation.expected_schema_fingerprint();
            definition.get("name").and_then(Value::as_str) == Some(operation.provider_name())
                && definition.get("contractVersion").and_then(Value::as_u64)
                    == Some(OPERATION_CONTRACT_VERSION)
                && declared_fingerprint == input_schema.map(schema_fingerprint).as_deref()
                && declared_fingerprint == Some(expected_fingerprint.as_str())
                && definition
                    .pointer("/inputSchema/type")
                    .and_then(Value::as_str)
                    == Some("object")
                && definition
                    .pointer("/outputSchema/type")
                    .and_then(Value::as_str)
                    == Some("object")
        });
        if !compatible {
            return Err(ApiError::Internal(
                "remote operation is unavailable or schema-incompatible".to_owned(),
            ));
        }
        let path = format!(
            "/api/operations/{}",
            HttpClient::encode_path_segment(operation.provider_name())
        );
        let response: OperationEnvelope = self
            .http
            .post_json_bounded(&path, params, MAX_CONTROL_PLANE_RESPONSE_BYTES)
            .await?;
        Ok(response.result)
    }

    /// Upload opaque bytes into an already-created principal-bound slot.
    pub async fn upload(
        &self,
        upload_id: &str,
        body: reqwest::Body,
        content_length: Option<u64>,
        content_type: &str,
    ) -> Result<Value, ApiError> {
        let path = format!("/uploads/{}", HttpClient::encode_path_segment(upload_id));
        self.http
            .put_body_bounded(
                &path,
                body,
                content_length,
                content_type,
                MAX_CONTROL_PLANE_RESPONSE_BYTES,
            )
            .await
    }
}

fn schema_fingerprint(schema: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(schema.to_string().as_bytes());
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::core::Auth;

    fn operation_definition(operation: Operation) -> Value {
        let input_schema = operation.expected_input_schema();
        json!({
            "name": operation.provider_name(),
            "contractVersion": 1,
            "schemaFingerprint": schema_fingerprint(&input_schema),
            "inputSchema": input_schema,
            "outputSchema": {"type":"object"}
        })
    }

    #[tokio::test]
    async fn executes_only_curated_operation_with_server_auth() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/operations"))
            .and(header("authorization", "Bearer server-secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "operations": [operation_definition(Operation::CandidatesList)]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/operations/depot.artifacts.list_candidates"))
            .and(header("authorization", "Bearer server-secret"))
            .and(body_json(json!({"query":"backup"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "result":{"candidates":[{"id":"candidate-1"}]}
            })))
            .mount(&server)
            .await;

        let http = HttpClient::new(
            server.uri(),
            Auth::Bearer {
                token: "server-secret".into(),
            },
        )
        .unwrap();
        let result = ArtifactControlClient::new(http)
            .execute(Operation::CandidatesList, &json!({"query":"backup"}))
            .await
            .unwrap();

        assert_eq!(result["candidates"][0]["id"], "candidate-1");
    }

    #[tokio::test]
    async fn rejects_oversized_provider_responses_before_json_decode() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/operations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "operations": [operation_definition(Operation::AuthorityStatus)]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/operations/depot.system.status"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![
                b' ';
                MAX_CONTROL_PLANE_RESPONSE_BYTES
                    + 1
            ]))
            .mount(&server)
            .await;
        let http = HttpClient::new(server.uri(), Auth::None).unwrap();
        let error = ArtifactControlClient::new(http)
            .execute(Operation::AuthorityStatus, &json!({}))
            .await
            .unwrap_err();
        assert!(matches!(error, ApiError::Decode(_)));
        assert!(error.to_string().contains("byte limit"));
    }

    #[tokio::test]
    async fn rejects_absent_or_schema_incompatible_operations_before_execution() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/operations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "operations": [{
                    "name": "depot.system.status",
                    "contractVersion": 1,
                    "schemaFingerprint": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                    "inputSchema": {"type":"string"},
                    "outputSchema": {"type":"object"}
                }]
            })))
            .mount(&server)
            .await;
        let http = HttpClient::new(server.uri(), Auth::None).unwrap();
        let error = ArtifactControlClient::new(http)
            .execute(Operation::AuthorityStatus, &json!({}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("schema-incompatible"));
    }

    #[tokio::test]
    async fn rejects_unversioned_or_unfingerprinted_operation_contracts() {
        let valid_fingerprint = Operation::AuthorityStatus.expected_schema_fingerprint();
        for definition in [
            json!({
                "name": "depot.system.status",
                "contractVersion": 2,
                "schemaFingerprint": valid_fingerprint.clone(),
                "inputSchema": Operation::AuthorityStatus.expected_input_schema(),
                "outputSchema": {"type":"object"}
            }),
            json!({
                "name": "depot.system.status",
                "contractVersion": 1,
                "schemaFingerprint": "not-a-sha256",
                "inputSchema": {"type":"object"},
                "outputSchema": {"type":"object"}
            }),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/api/operations"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(json!({"operations": [definition]})),
                )
                .mount(&server)
                .await;
            let http = HttpClient::new(server.uri(), Auth::None).unwrap();
            let error = ArtifactControlClient::new(http)
                .execute(Operation::AuthorityStatus, &json!({}))
                .await
                .unwrap_err();
            assert!(error.to_string().contains("schema-incompatible"));
        }
    }

    #[tokio::test]
    async fn rejects_a_well_formed_but_incorrect_schema_fingerprint() {
        let server = MockServer::start().await;
        let mut definition = operation_definition(Operation::AuthorityStatus);
        definition["schemaFingerprint"] = Value::String("a".repeat(64));
        Mock::given(method("GET"))
            .and(path("/api/operations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "operations": [definition]
            })))
            .mount(&server)
            .await;
        let http = HttpClient::new(server.uri(), Auth::None).unwrap();
        let error = ArtifactControlClient::new(http)
            .execute(Operation::AuthorityStatus, &json!({}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("schema-incompatible"));
    }

    #[tokio::test]
    async fn rejects_a_self_consistent_but_locally_unsupported_schema() {
        let server = MockServer::start().await;
        let incompatible_schema = json!({
            "type": "object",
            "properties": {"unexpectedRequiredInput": {"type": "string"}},
            "required": ["unexpectedRequiredInput"],
            "additionalProperties": false
        });
        let definition = json!({
            "name": Operation::AuthorityStatus.provider_name(),
            "contractVersion": 1,
            "schemaFingerprint": schema_fingerprint(&incompatible_schema),
            "inputSchema": incompatible_schema,
            "outputSchema": {"type":"object"}
        });
        Mock::given(method("GET"))
            .and(path("/api/operations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "operations": [definition]
            })))
            .mount(&server)
            .await;
        let http = HttpClient::new(server.uri(), Auth::None).unwrap();
        let error = ArtifactControlClient::new(http)
            .execute(Operation::AuthorityStatus, &json!({}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("schema-incompatible"));
    }

    #[test]
    fn seals_every_product_operation_to_an_explicit_provider_operation() {
        let operations = [
            Operation::ArtifactsList,
            Operation::ArtifactsGet,
            Operation::ArtifactsSearch,
            Operation::CandidatesList,
            Operation::CandidatesIntake,
            Operation::ArtifactsFollow,
            Operation::ArtifactsFork,
            Operation::ArtifactsSetPublication,
            Operation::ArtifactsSetLicense,
            Operation::SearchSkillsSh,
            Operation::SearchArd,
            Operation::SearchMarketplace,
            Operation::McpRegistryList,
            Operation::AcpRegistryList,
            Operation::AuthorityStatus,
            Operation::SourcesList,
            Operation::SourcesConfigure,
            Operation::SourcesDelete,
            Operation::SourcesRefresh,
            Operation::JobsStart,
            Operation::JobsList,
            Operation::JobsGet,
            Operation::JobsCancel,
            Operation::JobsRetry,
            Operation::UploadsCreate,
            Operation::UploadsGet,
            Operation::UploadsDelete,
            Operation::BundlesList,
            Operation::BundlesGet,
            Operation::BundlesCreate,
            Operation::BundlesAddArtifact,
            Operation::BundlesRemoveArtifact,
            Operation::BundlesSetVisibility,
            Operation::BundlesPublish,
            Operation::BundlesDelete,
        ];
        let names = operations.map(Operation::provider_name);
        let unique = names.into_iter().collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), operations.len());
        assert!(unique.iter().all(|name| name.starts_with("depot.")));
    }
}
