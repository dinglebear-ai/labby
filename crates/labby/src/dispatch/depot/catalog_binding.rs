//! Server-owned equality check between discovery and exact acquisition authority.
use super::network::{NetworkClient, NetworkPolicy, Operation, Secret};
use super::provider::Identity;
use crate::config::depot::{OpaqueEpoch, PUBLIC_ID, PublicReadBinding};
use tokio::sync::Semaphore;
use tokio::time::{Duration, Instant};

pub(crate) struct CatalogBinding {
    internal: NetworkClient,
    acquisition: NetworkClient,
    expected: OpaqueEpoch,
    slots: Semaphore,
}

impl CatalogBinding {
    pub(crate) fn new(
        binding: &PublicReadBinding,
        acquisition_endpoint: &str,
        token: &str,
    ) -> Result<Self, &'static str> {
        let local = Secret::local_bearer(PUBLIC_ID, &binding.endpoint, token)
            .map_err(|_| "invalid_catalog_read_credential")?;
        let internal = NetworkClient::local(PUBLIC_ID, &binding.endpoint, local)
            .map_err(|_| "invalid_catalog_read_endpoint")?;
        let mut origin = crate::config::depot::canonical_endpoint(acquisition_endpoint)
            .map_err(|_| "invalid_catalog_acquisition_endpoint")?;
        origin.set_path("/");
        let secret = Secret::bearer(origin.as_str(), token)
            .map_err(|_| "invalid_catalog_acquisition_credential")?;
        let acquisition =
            NetworkClient::new(origin.as_str(), Some(secret), NetworkPolicy::default())
                .map_err(|_| "invalid_catalog_acquisition_endpoint")?;
        Ok(Self {
            internal,
            acquisition,
            expected: binding.deployment_id.clone(),
            slots: Semaphore::new(16),
        })
    }

    #[cfg(test)]
    pub(crate) fn test_origins(internal: &str, acquisition: &str) -> Self {
        let client = |origin: &str| {
            NetworkClient::local(
                PUBLIC_ID,
                origin,
                Secret::local_bearer(PUBLIC_ID, origin, "fixture-catalog-token").unwrap(),
            )
            .unwrap()
        };
        Self {
            internal: client(internal),
            acquisition: client(acquisition),
            expected: "catalog".to_owned().try_into().unwrap(),
            slots: Semaphore::new(16),
        }
    }

    pub(crate) async fn verify(&self) -> Result<Identity, &'static str> {
        let _slot = self
            .slots
            .try_acquire()
            .map_err(|_| "catalog_binding_pending")?;
        let deadline = Instant::now() + Duration::from_secs(3);
        let (internal, acquisition) = tokio::try_join!(
            self.internal.call(Operation::Identity, None, deadline),
            self.acquisition.call(Operation::Identity, None, deadline),
        )
        .map_err(|_| "catalog_binding_unavailable")?;
        let internal = Identity::parse(internal).map_err(|_| "catalog_binding_incompatible")?;
        let acquisition =
            Identity::parse(acquisition).map_err(|_| "catalog_binding_incompatible")?;
        if internal.deployment_id != self.expected || !internal.same_authority(&acquisition) {
            return Err("catalog_authority_mismatch");
        }
        Ok(internal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn catalog_binding_checks_deployment_boot_and_credential_authority() {
        for changed in [
            None,
            Some("deploymentId"),
            Some("deploymentEpoch"),
            Some("authorityEpoch"),
        ] {
            let identity = json!({"contractVersion":"depot.discovery/v1", "deploymentId":"catalog", "deploymentEpoch":"boot", "authorityEpoch":"read", "listingEpoch":"1", "snapshotContinuations":true, "maxPageSize":200});
            let mut other = identity.clone();
            if let Some(field) = changed {
                other[field] = json!("different");
            }
            let response = |value: &serde_json::Value| {
                let body = value.to_string();
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                )
            };
            let (internal, _) = super::super::network_tests::tls_fixture(response(&identity)).await;
            let (acquisition, _) = super::super::network_tests::tls_fixture(response(&other)).await;
            let binding = CatalogBinding {
                internal,
                acquisition,
                expected: "catalog".to_owned().try_into().unwrap(),
                slots: Semaphore::new(16),
            };
            assert_eq!(binding.verify().await.is_ok(), changed.is_none());
        }
    }
}
