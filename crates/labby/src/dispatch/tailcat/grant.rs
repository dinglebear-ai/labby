//! Native credential custody for an approved browser session.
use super::{ApprovedPairing, PairingError};
use crate::access::AccessRuntime;
use labby_auth::{at_rest::TokenEncryptionKey, transport_grant::TransportGrantBinding};
use std::sync::Arc;

pub(super) struct GrantLease {
    runtime: Arc<AccessRuntime>,
    installation: String,
    credential_id: String,
    digest: [u8; 32],
    envelope: String,
    binding: TransportGrantBinding,
    armed: bool,
}

impl GrantLease {
    /// Resolve before taking the fair publication lease: the resolver itself
    /// reads that barrier, and recursively acquiring it can deadlock a writer.
    pub(super) async fn issue_checked(
        runtime: Arc<AccessRuntime>,
        manager: &labby_gateway::gateway::manager::GatewayManager,
        authority: &super::RequestAuthority,
    ) -> Result<Self, PairingError> {
        use labby_auth::ProductAccessGrantResolver as _;
        use labby_primitives::product_credential::ProductCredentialVerifier as _;
        let grant = authority
            .adapter
            .verify(&authority.source)
            .await
            .map_err(|_| PairingError)?;
        let current = authority
            .adapter
            .resolve(&grant)
            .await
            .map_err(|_| PairingError)?;
        let approved = &authority.approved;
        approved.authorize(
            approved.origin(),
            approved.generation(),
            approved.upstream(),
            &current,
            u64::try_from(labby_auth::util::now_unix()).map_err(|_| PairingError)?,
        )?;
        let lease = manager
            .acquire_published_bootstrap_policy_lease(&current.loadout_id, &current.route_id)
            .await
            .map_err(|_| PairingError)?;
        let epoch = runtime
            .reconcile_project_policy(current.project_id.clone(), lease.policy_fingerprint())
            .await
            .map_err(|_| PairingError)?;
        if epoch != current.loadout_generation
            || epoch != current.catalog_generation
            || epoch != current.route_generation
            || lease.resource() != current.resource
            || lease.audience() != current.audience
        {
            return Err(PairingError);
        }
        // Keep the publication guard through the durable issuance transaction.
        Self::issue(runtime, approved, &authority.key).await
    }
    /// Caller holds the published-policy lease and has freshly resolved the
    /// approval's source. The access transaction rechecks durable authority.
    pub(super) async fn issue(
        runtime: Arc<AccessRuntime>,
        approved: &ApprovedPairing,
        key: &TokenEncryptionKey,
    ) -> Result<Self, PairingError> {
        use crate::access::{
            IssueCredentialInput, SecurityAdmission, admit_credential_issue_attempt,
            credential_generation_sql,
        };
        use base64::Engine as _;
        use sha2::{Digest as _, Sha256};
        let now = labby_auth::util::now_unix();
        let now_unsigned = u64::try_from(now).map_err(|_| PairingError)?;
        let source = approved.source();
        approved.authorize(
            approved.origin(),
            approved.generation(),
            approved.upstream(),
            source,
            now_unsigned,
        )?;
        let mut scopes: Vec<String> = source
            .scopes
            .iter()
            .filter(|s| matches!(s.as_str(), "lab" | "lab:read"))
            .cloned()
            .collect();
        scopes.sort();
        scopes.dedup();
        if scopes.is_empty() {
            return Err(PairingError);
        }
        if !matches!(
            admit_credential_issue_attempt(&runtime, &source.credential_id, now).await,
            SecurityAdmission::Admitted
        ) {
            return Err(PairingError);
        }
        let mut secret = [0; 32];
        let mut idempotency_digest = [0; 32];
        getrandom::fill(&mut secret).map_err(|_| PairingError)?;
        getrandom::fill(&mut idempotency_digest).map_err(|_| PairingError)?;
        let credential_id = format!("tailcat-{}", uuid::Uuid::new_v4());
        let wire = format!(
            "{}{}_{}",
            labby_primitives::product_credential::PRODUCT_CREDENTIAL_PREFIX,
            credential_id,
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret)
        );
        let digest: [u8; 32] = Sha256::digest(wire.as_bytes()).into();
        let binding = TransportGrantBinding {
            installation: source.installation_id.clone(),
            principal: source.principal_id.clone(),
            peer: approved.peer().into(),
            origin: approved.origin().into(),
            resource: source.resource.clone(),
            upstream: approved.upstream().into(),
            generation: approved.generation().into(),
            expires_at: approved.expires_at(),
        };
        let scopes_json = serde_json::to_string(&scopes).map_err(|_| PairingError)?;
        let request_digest: [u8; 32] = Sha256::digest(
            serde_json::to_vec(&(
                "labby.tailcat.child-credential/v1",
                &binding,
                &scopes,
                digest,
            ))
            .map_err(|_| PairingError)?,
        )
        .into();
        let input = IssueCredentialInput {
            actor_credential_id: source.credential_id.clone(),
            actor_credential_generation: credential_generation_sql(source.credential_generation)
                .map_err(|_| PairingError)?,
            credential_id: credential_id.clone(),
            credential_digest: digest,
            credential_generation: 1,
            scopes_json,
            issued_at: now,
            expires_at: i64::try_from(approved.expires_at()).map_err(|_| PairingError)?,
            idempotency_digest,
            request_digest,
        };
        // Arm retirement before SQLite can commit. The tombstone also fences an
        // issuance that completes after its awaiting caller has been cancelled.
        let mut lease = Self {
            runtime,
            installation: source.installation_id.clone(),
            credential_id,
            digest,
            envelope: String::new(),
            binding,
            armed: true,
        };
        lease
            .runtime
            .issue_project_credential(input)
            .await
            .map_err(|_| PairingError)?;
        let credential = labby_primitives::product_credential::ProductCredential::parse(&wire)
            .map_err(|_| PairingError)?;
        lease.envelope =
            labby_auth::transport_grant::seal(key, &credential, &lease.binding, now_unsigned)
                .map_err(|_| PairingError)?;
        Ok(lease)
    }
    pub(super) fn envelope(&self) -> &str {
        &self.envelope
    }
    #[cfg(test)]
    pub(super) fn binding(&self) -> TransportGrantBinding {
        self.binding.clone()
    }
    pub(super) async fn revoke(mut self) -> Result<(), PairingError> {
        retire(
            &self.runtime,
            &self.installation,
            &self.credential_id,
            self.digest,
        )
        .await?;
        self.armed = false;
        Ok(())
    }
}

async fn retire(
    runtime: &AccessRuntime,
    installation: &str,
    credential_id: &str,
    digest: [u8; 32],
) -> Result<(), PairingError> {
    let store = runtime.store().await.map_err(|_| PairingError)?;
    store
        .tombstone_access_artifact(
            installation.into(),
            "credential".into(),
            credential_id.into(),
            digest,
            1,
            "tailcat_session_closed".into(),
            labby_auth::util::now_unix(),
        )
        .await
        .map_err(|_| PairingError)?;
    Ok(())
}

impl Drop for GrantLease {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(
                phase = "tailcat_grant_retire",
                "native cleanup runtime unavailable"
            );
            return;
        };
        let runtime = self.runtime.clone();
        let installation = self.installation.clone();
        let credential_id = self.credential_id.clone();
        let digest = self.digest;
        let _cleanup = handle.spawn(async move {
            if !matches!(
                tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    retire(&runtime, &installation, &credential_id, digest)
                )
                .await,
                Ok(Ok(()))
            ) {
                tracing::warn!(
                    phase = "tailcat_grant_retire",
                    "native credential retirement incomplete"
                );
            }
        });
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use labby_primitives::product_credential::ProductCredentialVerifier as _;
    #[cfg(feature = "tailcat")]
    #[tokio::test]
    async fn actual_protected_projection_initializes_and_refuses_operator_tools() {
        use base64::Engine as _;
        use tower::ServiceExt as _;
        let (directory, runtime, adapter, approved, manager) =
            super::super::testing::fixture_with_lifetime(12).await;
        let authority = Arc::new(super::super::RequestAuthority {
            adapter: adapter.clone(),
            approved,
            key: Arc::new(TokenEncryptionKey::from_encoded(&"01".repeat(32)).unwrap()),
            source: labby_primitives::product_credential::ProductCredential::parse(&format!(
                "lby_pc_v1_source_{}",
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0xA5; 32])
            ))
            .unwrap(),
        });
        let lease = GrantLease::issue_checked(runtime.clone(), &manager, &authority)
            .await
            .unwrap();
        let router = super::super::testing::protected_projection(
            &directory,
            runtime,
            adapter,
            authority.clone(),
            manager,
        )
        .await;
        let body = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params": {
            "protocolVersion":"2025-03-26", "capabilities":{}, "clientInfo":{"name":"tailcat-fixture","version":"1"}
        }});
        let now = u64::try_from(labby_auth::util::now_unix()).unwrap();
        let child = labby_auth::transport_grant::open(
            &authority.key,
            lease.envelope(),
            &lease.binding(),
            now,
        )
        .unwrap();
        let mut wrong_peer = lease.binding();
        wrong_peer.peer = format!("nodekey:{}", "2".repeat(64));
        let wrong_peer =
            labby_auth::transport_grant::seal(&authority.key, &child, &wrong_peer, now).unwrap();
        let response = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("authorization", format!("Bearer {wrong_peer}"))
                    .header("origin", authority.approved.origin())
                    .header("labby-tailcat-generation", authority.approved.generation())
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
        let wrong_origin = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("authorization", format!("Bearer {}", lease.envelope()))
                    .header("origin", "https://other.example")
                    .header("labby-tailcat-generation", authority.approved.generation())
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(wrong_origin.status(), axum::http::StatusCode::UNAUTHORIZED);
        let response = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("authorization", format!("Bearer {}", lease.envelope()))
                    .header("origin", authority.approved.origin())
                    .header("labby-tailcat-generation", authority.approved.generation())
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let session = response.headers().get("mcp-session-id").cloned();
        let bytes = axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap();
        assert!(std::str::from_utf8(&bytes).unwrap().contains("serverInfo"));
        let mut initialized = axum::http::Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("authorization", format!("Bearer {}", lease.envelope()))
            .header("origin", authority.approved.origin())
            .header("labby-tailcat-generation", authority.approved.generation())
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        if let Some(session) = &session {
            initialized = initialized.header("mcp-session-id", session);
        }
        let initialized = router
            .clone()
            .oneshot(
                initialized
                    .body(axum::body::Body::from(
                        serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(initialized.status(), axum::http::StatusCode::ACCEPTED);
        let mut request = axum::http::Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("authorization", format!("Bearer {}", lease.envelope()))
            .header("origin", authority.approved.origin())
            .header("labby-tailcat-generation", authority.approved.generation())
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        if let Some(session) = session {
            request = request.header("mcp-session-id", session);
        }
        let response = router.clone().oneshot(request.body(axum::body::Body::from(serde_json::json!({
            "jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"gateway","arguments":{"action":"status"}}
        }).to_string())).unwrap()).await.unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(text.contains("gateway"));
        assert!(text.contains("error") || text.contains("isError\":true"));
        let remaining = authority
            .approved
            .expires_at()
            .saturating_sub(labby_auth::util::now_unix() as u64);
        tokio::time::sleep(std::time::Duration::from_secs(remaining + 1)).await;
        let expired = router
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("authorization", format!("Bearer {}", lease.envelope()))
                    .header("origin", authority.approved.origin())
                    .header("labby-tailcat-generation", authority.approved.generation())
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(expired.status(), axum::http::StatusCode::UNAUTHORIZED);
        lease.revoke().await.unwrap();
    }

    #[tokio::test]
    async fn issuance_holds_current_policy_and_rejects_changed_publication() {
        use base64::Engine as _;
        let (_directory, runtime, adapter, approved, manager) =
            super::super::testing::fixture_with_manager().await;
        let authority = super::super::RequestAuthority {
            adapter,
            approved,
            key: Arc::new(TokenEncryptionKey::from_encoded(&"01".repeat(32)).unwrap()),
            source: labby_primitives::product_credential::ProductCredential::parse(&format!(
                "lby_pc_v1_source_{}",
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0xA5; 32])
            ))
            .unwrap(),
        };
        let lease = GrantLease::issue_checked(runtime.clone(), &manager, &authority)
            .await
            .unwrap();
        lease.revoke().await.unwrap();
        manager
            .seed_config_unchecked_for_tests(
                crate::config::LabConfig::default().to_gateway_config(),
            )
            .await;
        assert!(
            GrantLease::issue_checked(runtime, &manager, &authority)
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn native_child_is_short_lived_sealed_and_revoked_on_explicit_stop() {
        let (_directory, runtime, adapter, approved) = super::super::testing::fixture().await;
        let key = TokenEncryptionKey::from_encoded(&"01".repeat(32)).unwrap();
        let lease = GrantLease::issue(runtime, &approved, &key).await.unwrap();
        let credential = labby_auth::transport_grant::open(
            &key,
            lease.envelope(),
            &lease.binding(),
            labby_auth::util::now_unix() as u64,
        )
        .unwrap();
        let current = adapter.verify(&credential).await.unwrap();
        assert!(current.expires_at <= approved.expires_at());
        assert!(!current.scopes.iter().any(|s| s == "lab:admin"));
        lease.revoke().await.unwrap();
        assert!(adapter.verify(&credential).await.is_err());
    }
    #[tokio::test]
    async fn dropped_session_retires_its_native_child() {
        let (_directory, runtime, adapter, approved) = super::super::testing::fixture().await;
        let key = TokenEncryptionKey::from_encoded(&"01".repeat(32)).unwrap();
        let lease = GrantLease::issue(runtime, &approved, &key).await.unwrap();
        let credential = labby_auth::transport_grant::open(
            &key,
            lease.envelope(),
            &lease.binding(),
            labby_auth::util::now_unix() as u64,
        )
        .unwrap();
        drop(lease);
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while adapter.verify(&credential).await.is_ok() {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await
            }
        })
        .await
        .unwrap();
    }
}
