//! Durable Code Mode pause/resume support.
//!
//! This module is the `labby`-binary home for the durable-execution log that
//! backs Code Mode's mid-script human-in-the-loop pause/resume. It is a faithful
//! port of Cloudflare `agents`' `CodemodeRuntime` durable-execution model
//! (`packages/codemode/src/runtime.ts`), split across Labby's crate boundary:
//!
//! - The storage-neutral [`CodeModeDecider`](labby_codemode::host) trait lives
//!   in `labby-codemode` next to `CodeModeHost`.
//! - The SQLite store ([`sqlite_pauses`]) and the `decide()`/`record_result()`
//!   port ([`decider`]) live here in the binary crate — SQLite must live at the
//!   top of the dependency graph (`labby → labby-gateway → labby-codemode`).
//! - `GatewayManager` (labby-gateway) receives an injected
//!   `Arc<dyn CodeModeDecider>`; `None` preserves today's no-pause behavior.
//!
//! Gated `#[cfg(feature = "gateway")]` because it depends on
//! `labby_codemode::redact_trace_value`, only present under `gateway`.

pub mod decider;
pub mod sqlite_pauses;

/// Milliseconds since the Unix epoch, saturating to 0 on clock error.
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Default abandoned-pause TTL (24h, matching Cloudflare's `DEFAULT_PAUSED_TTL_MS`;
/// `runtime.ts:156`).
pub(crate) const DEFAULT_PAUSED_TTL_MS: i64 = 24 * 60 * 60 * 1000;

/// Configured pause TTL in ms (`LABBY_CODE_MODE_PAUSE_TTL_MS`, default 24h).
pub(crate) fn pause_ttl_ms() -> i64 {
    std::env::var("LABBY_CODE_MODE_PAUSE_TTL_MS")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_PAUSED_TTL_MS)
}
