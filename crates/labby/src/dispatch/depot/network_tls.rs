//! Bounded native trust setup for pinned Depot connectors.
use super::network::NetworkError;
use std::sync::{Arc, LazyLock};
#[cfg(test)]
use std::time::Duration;
use tokio::sync::Semaphore;

static TLS_SETUP_CAPACITY: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(4)));
type TlsBuilder = hyper_rustls::HttpsConnectorBuilder<hyper_rustls::builderstates::WantsSchemes>;

async fn prepare_tls<F>(capacity: Arc<Semaphore>, build: F) -> Result<TlsBuilder, NetworkError>
where
    F: FnOnce() -> Result<TlsBuilder, NetworkError> + Send + 'static,
{
    let permit = capacity
        .acquire_owned()
        .await
        .map_err(|_| NetworkError::Unavailable)?;
    tokio::task::spawn_blocking(move || {
        // A cancelled caller cannot admit another setup while this worker lives.
        let _permit = permit;
        build()
    })
    .await
    .map_err(|_| NetworkError::Unavailable)?
}

pub(super) async fn connector_tls(
    local: bool,
    #[cfg(test)] test_tls: Option<rustls::ClientConfig>,
    #[cfg(test)] test_hook: Option<Arc<dyn Fn() + Send + Sync>>,
) -> Result<TlsBuilder, NetworkError> {
    if local {
        // Canonical host-managed endpoints are plain HTTP. The connector's
        // unused TLS branch must not enumerate the native trust store.
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|_| NetworkError::Unavailable)?
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
        return Ok(hyper_rustls::HttpsConnectorBuilder::new().with_tls_config(tls));
    }
    prepare_tls(TLS_SETUP_CAPACITY.clone(), move || {
        #[cfg(test)]
        if let Some(hook) = test_hook {
            hook();
        }
        #[cfg(test)]
        if let Some(tls) = test_tls {
            return Ok(hyper_rustls::HttpsConnectorBuilder::new().with_tls_config(tls));
        }
        hyper_rustls::HttpsConnectorBuilder::new()
            .with_provider_and_native_roots(Arc::new(rustls::crypto::ring::default_provider()))
            .map_err(|_| NetworkError::Unavailable)
    })
    .await
}

#[cfg(test)]
mod tls_capacity_tests {
    use super::*;

    #[tokio::test]
    async fn cancelled_tls_setup_retains_blocking_capacity_until_worker_finishes() {
        let capacity = Arc::new(Semaphore::new(1));
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let slots = capacity.clone();
        let task = tokio::spawn(prepare_tls(slots, move || {
            let _ = entered_tx.send(());
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            Err(NetworkError::Unavailable)
        }));
        entered_rx.await.unwrap();
        task.abort();
        drop(task.await);
        assert_eq!(capacity.available_permits(), 0);
        release_tx.send(()).unwrap();
        let permit = tokio::time::timeout(Duration::from_secs(1), capacity.acquire())
            .await
            .unwrap()
            .unwrap();
        drop(permit);
        assert_eq!(capacity.available_permits(), 1);
    }
}
