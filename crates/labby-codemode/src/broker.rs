//! `CodeModeBroker<H>`: the per-request driver that wires the JS execution
//! kernel to a [`CodeModeHost`]. A fresh broker is constructed per request, so
//! its run-scoped state (the captured mcp-ui widget link) is naturally scoped
//! to one execution.

use crate::host::CodeModeHost;
use crate::types::UiLink;

pub(crate) fn lab_action_unknown_tool_hint() -> String {
    "Code Mode handles host-provided tools only. For Lab actions, call the native \
     Lab service tool with arguments={action:<dotted.action>, params:{...}}. \
     Example: radarr(arguments={action:\"movie.search\", params:{query:\"Matrix\"}})."
        .to_string()
}

/// Drives a single Code Mode execution against an injected [`CodeModeHost`].
///
/// `host` is `Option` so the standalone/no-host path (some tests) can construct
/// a broker that spawns a one-shot runner with an empty catalog and no tool
/// source.
pub struct CodeModeBroker<'a, H: CodeModeHost> {
    pub(crate) host: Option<&'a H>,
    /// Run-scoped sink for the last MCP Apps (mcp-ui) widget link seen during
    /// this execution. Recorded by the host at the `call_tool` boundary
    /// (last-wins), then surfaced in the Code Mode result.
    pub(crate) ui_capture: std::sync::Arc<std::sync::Mutex<Option<UiLink>>>,
    /// Durable-run execution id for the pause-capable path. `Some` only when the
    /// binary started a durable run (`store.begin`) before driving; threaded
    /// into `ExecCtx` at each `call_tool` so the host's decider can journal.
    /// `None` ⇒ the write-free path (CLI, pre-confirmed, no-decider, tests).
    pub(crate) execution_id: Option<String>,
}

impl<'a, H: CodeModeHost> CodeModeBroker<'a, H> {
    #[must_use]
    pub fn new(host: Option<&'a H>) -> Self {
        Self {
            host,
            ui_capture: std::sync::Arc::new(std::sync::Mutex::new(None)),
            execution_id: None,
        }
    }

    /// Attach a durable-run execution id (pause-capable path). Builder-style.
    #[must_use]
    pub fn with_execution_id(mut self, execution_id: Option<String>) -> Self {
        self.execution_id = execution_id;
        self
    }
}
