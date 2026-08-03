//! Shared private MCP protocol identifiers.
//!
//! These values are consumed by both the feature-independent Labby MCP
//! handler and the optional gateway relay runtime. Keeping them in the leaf
//! primitives crate prevents either side from depending on the other merely
//! to agree on wire-level names.

/// Private MCP `_meta` key correlating stateless HTTP cancellation posts with
/// the original relayed request. The value is a random per-request token.
pub const MCP_RELAY_CANCELLATION_TOKEN_META_KEY: &str =
    "ai.dinglebear.labby/relayCancellationToken";

/// Labby-private request used alongside standard cancellation when an rmcp
/// stateless HTTP hop hides or rewrites request IDs.
pub const MCP_RELAY_CANCELLATION_REQUEST_METHOD: &str = "ai.dinglebear.labby/relay-cancel";
