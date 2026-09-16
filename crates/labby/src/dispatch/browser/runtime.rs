//! Process-scoped browser runtime shared by every Labby surface.

use std::sync::Arc;
use tokio::sync::OnceCell;

use labby_browser::BrowserBridge;

use crate::dispatch::error::ToolError;
use crate::installation::InstallationPaths;

static BROWSER_BRIDGE: OnceCell<Arc<BrowserBridge>> = OnceCell::const_new();

/// Only a validated browser-extension socket may create the owning runtime.
pub(crate) async fn initialize_browser_bridge() -> Result<Arc<BrowserBridge>, ToolError> {
    let path = InstallationPaths::resolve()
        .map_err(|error| unavailable(error.to_string()))?
        .root()
        .join("browser/browser.db");
    open_browser_bridge(&BROWSER_BRIDGE, &path).await
}

fn unavailable(message: String) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "browser_unavailable".to_string(),
        message,
    }
}

// Cache only a successfully acquired owner. A failed attempt must remain
// retryable after the operator repairs storage or releases another owner.
async fn open_browser_bridge(
    cell: &OnceCell<Arc<BrowserBridge>>,
    path: &std::path::Path,
) -> Result<Arc<BrowserBridge>, ToolError> {
    cell.get_or_try_init(|| async {
        BrowserBridge::open(path)
            .await
            .map(Arc::new)
            .map_err(|error| unavailable(error.to_string()))
    })
    .await
    .map(Arc::clone)
}

/// Other surfaces must target the daemon that owns the extension connection.
pub async fn browser_bridge() -> Result<Arc<BrowserBridge>, ToolError> {
    BROWSER_BRIDGE.get().map(Arc::clone).ok_or_else(|| unavailable(
        "Connect the browser extension to this Labby server first. Other clients must target the owning server; a separate CLI or stdio process does not own its browser connections.".to_string()
    ))
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn uninitialized_process_does_not_create_a_second_runtime() {
        let error = browser_bridge().await.err().expect("no owning daemon");
        assert!(error.to_string().contains("owning server"));
        assert!(super::BROWSER_BRIDGE.get().is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn repaired_browser_storage_retries_initialization_without_process_restart() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("browser.db");
        let cell = tokio::sync::OnceCell::new();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(super::open_browser_bridge(&cell, &path).await.is_err());
        assert!(cell.get().is_none());
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let first = super::open_browser_bridge(&cell, &path).await.unwrap();
        let second = super::open_browser_bridge(&cell, &path).await.unwrap();
        assert!(std::sync::Arc::ptr_eq(&first, &second));
    }

    use super::browser_bridge;
}
