//! Process-scoped browser runtime shared by every Labby surface.

use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::OnceCell;

use labby_browser::BrowserBridge;

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::lab_home;

static BROWSER_BRIDGE: OnceCell<Result<Arc<BrowserBridge>, String>> = OnceCell::const_new();

pub async fn browser_bridge() -> Result<Arc<BrowserBridge>, ToolError> {
    BROWSER_BRIDGE
        .get_or_init(|| async {
            let path: PathBuf = lab_home().join("browser").join("browser.db");
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
