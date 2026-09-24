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

/// Prefix every failed upstream `resources/list` error carries.
///
/// This string is a classification contract, not merely a message: operator
/// surfaces match on it to decide that an *optional* capability failed rather
/// than that the upstream is down, which is the difference between rendering a
/// warning and rendering the server as disconnected. The producers live in the
/// gateway runtime while two classifiers — the gateway projection and the
/// doctor gateway check — do not, and the doctor compiles in feature slices
/// that have no gateway crate at all. So the constant lives here, in the leaf
/// both sides already depend on, rather than in either of them.
pub const UPSTREAM_RESOURCE_LISTING_ERROR_PREFIX: &str = "failed to list resources from upstream:";

/// Prompt-listing counterpart of [`UPSTREAM_RESOURCE_LISTING_ERROR_PREFIX`].
pub const UPSTREAM_PROMPT_LISTING_ERROR_PREFIX: &str = "failed to list prompts from upstream:";
