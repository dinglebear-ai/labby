//! Process-scoped browser runtime shared by every Labby surface.

use std::sync::Arc;
use tokio::sync::OnceCell;

use labby_browser::BrowserBridge;

use crate::dispatch::error::ToolError;
use crate::installation::InstallationPaths;

static BROWSER_BRIDGE: OnceCell<Result<Arc<BrowserBridge>, String>> = OnceCell::const_new();

/// Only a validated browser-extension socket may create the owning runtime.
pub(crate) async fn initialize_browser_bridge() -> Result<Arc<BrowserBridge>, ToolError> {
    BROWSER_BRIDGE
        .get_or_init(|| async {
            let path = InstallationPaths::resolve()
                .map_err(|error| error.to_string())?
                .root()
                .join("browser/browser.db");
            BrowserBridge::open(&path)
                .await
                .map(Arc::new)
                .map_err(|error| error.to_string())
        })
        .await
        .as_ref()
        .map(Arc::clone)
        .map_err(|message| ToolError::Sdk {
            sdk_kind: "browser_unavailable".to_string(),
            message: message.clone(),
        })
}

/// Other surfaces must target the daemon that owns the extension connection.
pub async fn browser_bridge() -> Result<Arc<BrowserBridge>, ToolError> {
    BROWSER_BRIDGE
        .get()
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "browser_unavailable".to_string(),
            message: "Connect the browser extension to this Labby server first. Other clients must target the owning server; a separate CLI or stdio process does not own its browser connections.".to_string(),
        })?
        .as_ref()
        .map(Arc::clone)
        .map_err(|message| ToolError::Sdk {
            sdk_kind: "browser_unavailable".to_string(),
            message: message.clone(),
        })
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn uninitialized_process_does_not_create_a_second_runtime() {
        let error = browser_bridge().await.err().expect("no owning daemon");
        assert!(error.to_string().contains("owning server"));
        assert!(super::BROWSER_BRIDGE.get().is_none());
    }

    use super::browser_bridge;
}
