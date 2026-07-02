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

pub mod sqlite_pauses;
