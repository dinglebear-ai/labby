//! Uncached native authority checks for every restricted transport request.
use super::{ApprovedPairing, PairingError};
use crate::access::AccessCredentialAdapter;
use labby_auth::{
    ProductAccessGrantResolver as _, at_rest::TokenEncryptionKey,
    transport_grant::TransportGrantBinding,
};
use labby_primitives::product_credential::{ProductCredential, ProductCredentialVerifier as _};

/// Native custody only. Neither credential nor encryption key is serialized.
pub(crate) struct RequestAuthority {
    pub(crate) adapter: std::sync::Arc<AccessCredentialAdapter>,
    pub(crate) source: ProductCredential,
    pub(crate) approved: ApprovedPairing,
    pub(crate) key: std::sync::Arc<TokenEncryptionKey>,
}

impl RequestAuthority {
    #[cfg(all(test, feature = "tailcat", unix))]
    pub(super) fn binding(&self) -> TransportGrantBinding {
        let source = self.approved.source();
        TransportGrantBinding {
            installation: source.installation_id.clone(),
            principal: source.principal_id.clone(),
            peer: self.approved.peer().into(),
            origin: self.approved.origin().into(),
            resource: source.resource.clone(),
            upstream: self.approved.upstream().into(),
            generation: self.approved.generation().into(),
            expires_at: self.approved.expires_at(),
        }
    }
    pub(crate) async fn authorize(
        &self,
        envelope: &str,
        origin: &str,
        generation: &str,
    ) -> Result<ProductCredential, PairingError> {
        authorize(
            &self.adapter,
            &self.source,
            &self.approved,
            &self.key,
            envelope,
            origin,
            generation,
        )
        .await
    }
}

/// Opening the browser envelope never establishes authority by itself. Both the
/// original operator credential and its restricted child must still be current.
pub(super) async fn authorize(
    adapter: &AccessCredentialAdapter,
    source: &ProductCredential,
    approved: &ApprovedPairing,
    key: &TokenEncryptionKey,
    envelope: &str,
    origin: &str,
    generation: &str,
) -> Result<ProductCredential, PairingError> {
    let now = u64::try_from(labby_auth::util::now_unix()).map_err(|_| PairingError)?;
    let source_grant = adapter.verify(source).await.map_err(|_| PairingError)?;
    let current = adapter
        .resolve(&source_grant)
        .await
        .map_err(|_| PairingError)?;
    approved.authorize(origin, generation, approved.upstream(), &current, now)?;
    let binding = TransportGrantBinding {
        installation: current.installation_id.clone(),
        principal: current.principal_id.clone(),
        peer: approved.peer().into(),
        origin: approved.origin().into(),
        resource: current.resource.clone(),
        upstream: approved.upstream().into(),
        generation: approved.generation().into(),
        expires_at: approved.expires_at(),
    };
    let child = labby_auth::transport_grant::open(key, envelope, &binding, now)
        .map_err(|_| PairingError)?;
    let child_grant = adapter.verify(&child).await.map_err(|_| PairingError)?;
    let child_binding = adapter
        .resolve(&child_grant)
        .await
        .map_err(|_| PairingError)?;
    let mut expected = current;
    expected.credential_id = child.credential_id().into();
    expected.credential_generation = 1;
    expected.expires_at = approved.expires_at();
    expected
        .scopes
        .retain(|scope| matches!(scope.as_str(), "lab" | "lab:read"));
    expected.scopes.sort();
    expected.scopes.dedup();
    if child_binding != expected || expected.scopes.is_empty() {
        return Err(PairingError);
    }
    Ok(child)
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::grant::GrantLease;
    use super::*;
    use base64::Engine as _;

    #[tokio::test]
    async fn native_revocation_closes_existing_stream_and_listener() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let (_directory, runtime, adapter, approved) = super::super::testing::fixture().await;
        let key = std::sync::Arc::new(TokenEncryptionKey::from_encoded(&"01".repeat(32)).unwrap());
        let lease = GrantLease::issue(runtime, &approved, &key).await.unwrap();
        let source = ProductCredential::parse(&format!(
            "lby_pc_v1_source_{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0xA5; 32])
        ))
        .unwrap();
        let authority = std::sync::Arc::new(RequestAuthority {
            adapter,
            source,
            approved,
            key,
        });
        // This fixture tests stream ownership, not a successful MCP dispatch.
        let router = axum::Router::new().route(
            "/mcp",
            axum::routing::get(|| async {
                axum::body::Body::from_stream(futures::stream::pending::<
                    Result<bytes::Bytes, std::io::Error>,
                >())
            }),
        );
        let listener = crate::api::tailcat::RestrictedListener::start_authorized(
            router,
            authority,
            lease.envelope().to_owned(),
        )
        .await
        .unwrap();
        let mut stream = tokio::net::TcpStream::connect(listener.address())
            .await
            .unwrap();
        stream
            .write_all(b"GET /mcp HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut buffer = [0; 1024];
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), stream.read(&mut buffer))
                .await
                .unwrap()
                .unwrap()
                > 0
        );
        lease.revoke().await.unwrap();
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(7), stream.read(&mut buffer))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        assert!(
            tokio::net::TcpStream::connect(listener.address())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn revoking_the_original_operator_denies_a_still_current_child() {
        use sha2::{Digest as _, Sha256};
        let (_directory, runtime, adapter, approved) = super::super::testing::fixture().await;
        let key = TokenEncryptionKey::from_encoded(&"01".repeat(32)).unwrap();
        let lease = GrantLease::issue(runtime.clone(), &approved, &key)
            .await
            .unwrap();
        let wire = format!(
            "lby_pc_v1_source_{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0xA5; 32])
        );
        let source = ProductCredential::parse(&wire).unwrap();
        runtime
            .store()
            .await
            .unwrap()
            .tombstone_access_artifact(
                "machine".into(),
                "credential".into(),
                "source".into(),
                Sha256::digest(wire.as_bytes()).into(),
                1,
                "test_revocation".into(),
                labby_auth::util::now_unix(),
            )
            .await
            .unwrap();
        let child = labby_auth::transport_grant::open(
            &key,
            lease.envelope(),
            &lease.binding(),
            labby_auth::util::now_unix() as u64,
        )
        .unwrap();
        assert!(adapter.verify(&child).await.is_ok());
        assert!(
            authorize(
                &adapter,
                &source,
                &approved,
                &key,
                lease.envelope(),
                approved.origin(),
                approved.generation()
            )
            .await
            .is_err()
        );
        lease.revoke().await.unwrap();
    }

    #[tokio::test]
    async fn requests_require_current_native_authority_and_exact_browser_binding() {
        let (_directory, runtime, adapter, approved) = super::super::testing::fixture().await;
        let key = TokenEncryptionKey::from_encoded(&"01".repeat(32)).unwrap();
        let lease = GrantLease::issue(runtime, &approved, &key).await.unwrap();
        let source = ProductCredential::parse(&format!(
            "lby_pc_v1_source_{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0xA5; 32])
        ))
        .unwrap();
        assert!(
            authorize(
                &adapter,
                &source,
                &approved,
                &key,
                lease.envelope(),
                approved.origin(),
                approved.generation()
            )
            .await
            .is_ok()
        );
        for (origin, generation) in [
            ("https://evil.example", approved.generation()),
            (approved.origin(), "stale-generation"),
        ] {
            assert!(
                authorize(
                    &adapter,
                    &source,
                    &approved,
                    &key,
                    lease.envelope(),
                    origin,
                    generation
                )
                .await
                .is_err()
            );
        }
        let envelope = lease.envelope().to_owned();
        lease.revoke().await.unwrap();
        assert!(
            authorize(
                &adapter,
                &source,
                &approved,
                &key,
                &envelope,
                approved.origin(),
                approved.generation()
            )
            .await
            .is_err()
        );
    }
}
