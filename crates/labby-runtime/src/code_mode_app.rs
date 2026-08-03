use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Shared runtime state for Labby's explicit Code Mode MCP App surface.
///
/// The persisted source of truth lives in `CodeModeConfig::mcp_ui_enabled`.
/// This handle mirrors that value so every active MCP session observes config
/// changes immediately without rebuilding the server.
#[derive(Clone, Debug)]
pub struct CodeModeAppState {
    enabled: Arc<AtomicBool>,
}

impl Default for CodeModeAppState {
    fn default() -> Self {
        Self::new(true)
    }
}

impl CodeModeAppState {
    #[must_use]
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(enabled)),
        }
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// Set the gateway-wide state and return the previous value.
    pub fn set_enabled(&self, enabled: bool) -> bool {
        self.enabled.swap(enabled, Ordering::AcqRel)
    }
}

#[cfg(test)]
mod tests {
    use super::CodeModeAppState;

    #[test]
    fn cloned_handles_share_state() {
        let state = CodeModeAppState::new(true);
        let sibling = state.clone();

        assert!(state.set_enabled(false));
        assert!(!sibling.is_enabled());
    }
}
