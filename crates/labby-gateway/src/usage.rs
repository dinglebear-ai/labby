//! Gateway call-usage telemetry: a small SQLite-backed store recording every
//! tool/resource/prompt call proxied through the upstream pool, plus the
//! aggregation queries backing the `gateway.usage.*` actions.

pub mod store;
pub mod types;

pub use store::UsageStore;
pub use types::UpstreamCallRecord;
