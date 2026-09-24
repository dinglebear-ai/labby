//! Host-owned authority for caller-bound first-party OAuth from Code Mode.

use std::future::Future;
use std::pin::Pin;

use labby_runtime::error::ToolError;
use serde_json::Value;

use crate::gateway::manager::GatewayManager;

/// The product host validates the original transport identity and access
/// lease. Gateway runtime never interprets or persists the opaque token.
pub trait CodeModePersonalOauthProvider: Send + Sync {
    fn authorize<'a>(
        &'a self,
        manager: &'a GatewayManager,
        authority_token: &'a str,
        params: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send + 'a>>;
}
