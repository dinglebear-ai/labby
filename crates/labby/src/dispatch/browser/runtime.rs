//! Process-scoped browser runtime shared by every Labby surface.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use labby_browser::BrowserBridge;

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::lab_home;

static BROWSER_BRIDGE: OnceLock<Result<Arc<BrowserBridge>, String>> = OnceLock::new();

pub fn browser_bridge() -> Result<Arc<BrowserBridge>, ToolError> {
    BROWSER_BRIDGE
        .get_or_init(|| {
            let path: PathBuf = lab_home().join("browser").join("browser.db");
            BrowserBridge::open(&path)
                .map(Arc::new)
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map(Arc::clone)
        .map_err(|message| ToolError::Sdk {
            sdk_kind: "browser_unavailable".to_string(),
            message: message.clone(),
        })
}
