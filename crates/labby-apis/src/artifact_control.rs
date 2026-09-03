//! Typed client for Labby's remote Artifact control-plane authority.
//!
//! The provider operation names are deliberately sealed in this module. Product
//! dispatchers select from the sealed `Operation` enum instead of forwarding arbitrary remote
//! operation strings supplied by a caller.

use serde::Deserialize;
use serde_json::Value;

use crate::core::{ApiError, HttpClient};

const MAX_CONTROL_PLANE_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

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
}

#[derive(Debug, Deserialize)]
struct OperationEnvelope {
    result: Value,
}

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
        bytes: Vec<u8>,
        content_type: &str,
    ) -> Result<Value, ApiError> {
        let path = format!("/uploads/{}", HttpClient::encode_path_segment(upload_id));
        self.http
            .put_bytes_bounded(&path, bytes, content_type, MAX_CONTROL_PLANE_RESPONSE_BYTES)
            .await
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::core::Auth;

    #[tokio::test]
    async fn executes_only_curated_operation_with_server_auth() {
        let server = MockServer::start().await;
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
