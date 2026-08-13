//! Request-context, auth-subject, and scope/admin gate helpers.
//!
//! Extracted from `server.rs` (bead `lab-kvji.24.1.1`). Holds:
//! - inherent `impl LabMcpServer` request-context methods (Rust permits
//!   multiple inherent impl blocks for one struct across files; the trait
//!   impl stays single-file in `server.rs`),
//! - free auth-extraction helpers,
//! - the scope/admin gate fns (widened to `pub(crate)` per Revision 2 so
//!   `call_tool*`/resource helpers can call them — visibility change only,
//!   no logic change).

use axum::http::request::Parts;
use labby_auth::auth_context::AuthContext;
use rmcp::RoleServer;
use rmcp::service::RequestContext;
use sha2::{Digest, Sha256};

#[cfg(feature = "gateway")]
use crate::dispatch::gateway::code_mode::CodeModeSurface;
#[cfg(feature = "gateway")]
use crate::dispatch::upstream::types::UpstreamRuntimeOwner;
use crate::mcp::server::LabMcpServer;

#[cfg(feature = "gateway")]
pub(crate) use crate::api::oauth::oauth_upstream_subject_for_request;

pub(crate) fn redact_subject_for_logging(subject: &str) -> String {
    let digest = Sha256::digest(subject.as_bytes());
    format!("sub:{}", hex::encode(digest))[..16].to_string()
}

#[cfg(feature = "gateway")]
pub(crate) fn redacted_oauth_subject_label() -> &'static str {
    "[redacted]"
}

impl LabMcpServer {
    #[cfg(feature = "gateway")]
    pub(crate) fn code_mode_surface(&self) -> CodeModeSurface {
        CodeModeSurface::Mcp
    }

    pub(crate) fn request_subject<'a>(
        &self,
        context: &'a RequestContext<RoleServer>,
    ) -> Option<&'a str> {
        subject_from_extensions(&context.extensions)
    }

    pub(crate) fn request_subject_log_tag(&self, context: &RequestContext<RoleServer>) -> String {
        self.request_subject(context)
            .map(redact_subject_for_logging)
            .unwrap_or_default()
    }

    pub(crate) fn request_actor_key<'a>(
        &self,
        context: &'a RequestContext<RoleServer>,
    ) -> Option<&'a str> {
        actor_key_from_extensions(&context.extensions)
    }

    #[cfg(feature = "gateway")]
    pub(crate) fn request_runtime_owner(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> UpstreamRuntimeOwner {
        let subject = self.request_subject(context);
        crate::dispatch::gateway::shared::make_mcp_runtime_owner(subject)
    }

    #[cfg(feature = "gateway")]
    pub(crate) async fn oauth_upstream_configs(&self) -> Vec<crate::config::UpstreamConfig> {
        match &self.gateway_manager {
            Some(manager) => manager.oauth_upstream_configs().await,
            None => Vec::new(),
        }
    }

    #[cfg(feature = "gateway")]
    pub(crate) async fn route_scoped_oauth_upstream_configs(
        &self,
    ) -> Vec<crate::config::UpstreamConfig> {
        let mut configs = self.oauth_upstream_configs().await;
        configs.retain(|config| self.route_scope.allows_upstream(&config.name));
        configs
    }

    #[cfg(feature = "gateway")]
    pub(crate) async fn oauth_upstream_config(
        &self,
        upstream_name: &str,
    ) -> Option<crate::config::UpstreamConfig> {
        match &self.gateway_manager {
            Some(manager) => manager.oauth_upstream_config(upstream_name).await,
            None => None,
        }
    }
}

/// Return the capability snapshot for the current request.
///
/// Even an empty capability object requires a relay connection: progress and
/// cancellation are request-scoped protocol behavior, not optional client
/// capabilities. Legacy requests without modern metadata are represented by an
/// honest empty capability set rather than falling back to connection history.
pub(crate) fn forwardable_client_capabilities(
    meta: Option<&rmcp::model::RequestMetaObject>,
) -> Option<rmcp::model::ClientCapabilities> {
    Some(
        meta.and_then(rmcp::model::RequestMetaObject::client_capabilities)
            .unwrap_or_default(),
    )
}

pub(crate) fn subject_from_extensions(extensions: &rmcp::model::Extensions) -> Option<&str> {
    auth_context_from_extensions(extensions).map(|auth| auth.sub.as_str())
}

pub(crate) fn actor_key_from_extensions(extensions: &rmcp::model::Extensions) -> Option<&str> {
    auth_context_from_extensions(extensions).and_then(|auth| auth.actor_key.as_deref())
}

pub(crate) fn auth_context_from_extensions(
    extensions: &rmcp::model::Extensions,
) -> Option<&AuthContext> {
    let parts = extensions.get::<Parts>()?;
    parts.extensions.get::<AuthContext>()
}

pub(crate) fn tool_execute_scope_allowed(auth: Option<&AuthContext>) -> bool {
    auth.is_none_or(|auth| {
        auth.scopes
            .iter()
            .any(|scope| matches!(scope.as_str(), "lab" | "lab:admin"))
    })
}

/// Returns `true` when the caller is allowed to read Code Mode resources.
///
/// Code Mode app resources require at least `lab:read`; executable Code Mode
/// calls require the stronger `lab` or `lab:admin`.
/// `None` auth means stdio transport — trusted by design (no per-request AuthContext).
pub(crate) fn code_mode_read_scope_allowed(auth: Option<&AuthContext>) -> bool {
    auth.is_none_or(|auth| {
        auth.scopes
            .iter()
            .any(|scope| matches!(scope.as_str(), "lab:read" | "lab" | "lab:admin"))
    })
}

/// Whether an absent `AuthContext` may be read as "trusted local stdio".
///
/// The stdio trust model treats a missing per-request auth context as a local
/// operator at a terminal. That inference is only sound on a transport that
/// *would* have carried auth for a remote caller. The in-process peer
/// (`mcp/in_process_peer.rs`) is served over `tokio::io::duplex`, which has no
/// HTTP layer, so `auth_context_from_extensions` finds no `Parts` and resolves
/// to `None` for **every** caller — including a remote, non-admin OAuth caller
/// who reached it through Code Mode. On that transport an absent context proves
/// nothing and must not be trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AbsentAuth {
    /// No auth context means a local stdio caller. Applies to transports that
    /// inject one for every authenticated remote caller.
    TrustedLocal,
    /// No auth context proves nothing about the caller. Applies to the
    /// in-process peer, whose transport cannot carry auth at all.
    Untrusted,
}

pub(crate) fn tool_execute_builtin_action_allowed(
    entry: &crate::registry::RegisteredService,
    action: &str,
    auth: Option<&AuthContext>,
    absent_auth: AbsentAuth,
) -> bool {
    let bare = action
        .strip_prefix(&format!("{}.", entry.name))
        .unwrap_or(action);
    if entry.name == "setup" && crate::dispatch::setup::LOCAL_ONLY_ACTIONS.contains(&bare) {
        // These mint credentials or ask the host to probe a caller-selected
        // URL, so they are for trusted local stdio only. MCP-over-HTTP always
        // carries an AuthContext, and the in-process peer can never prove it is
        // local — both are refused.
        return auth.is_none() && absent_auth == AbsentAuth::TrustedLocal;
    }
    if !builtin_action_requires_admin(entry, action) {
        return true;
    }
    // INTENTIONAL ASYMMETRY with the HTTP API gate (`api/services/gateway.rs`,
    // which uses `is_some_and` — absent auth = DENIED). Here absent auth is
    // allowed *only* on a transport where it genuinely implies local stdio.
    // Remote MCP-over-HTTP cannot reach here unauthenticated because
    // `cli/serve.rs` refuses to bind a non-loopback address without auth
    // configured, and the `/mcp` route carries the bearer/OAuth layer when auth
    // is configured. The in-process peer is the transport that broke that
    // inference, which is why `absent_auth` is a parameter rather than a
    // constant. Do NOT widen this without proving the new transport injects an
    // AuthContext for every authenticated caller.
    match auth {
        None => absent_auth == AbsentAuth::TrustedLocal,
        Some(auth) => auth.scopes.iter().any(|scope| scope == "lab:admin"),
    }
}

pub(crate) fn builtin_action_requires_admin(
    entry: &crate::registry::RegisteredService,
    action: &str,
) -> bool {
    // Catalog-driven metadata is the single source of truth for every
    // registered service. Keeping an allow-list here caused newly registered
    // services (notably Doctor) to silently bypass their admin metadata.
    let service_prefix = format!("{}.", entry.name);
    let bare = action.strip_prefix(&service_prefix).unwrap_or(action);
    if bare == "help" || bare == "schema" {
        return false;
    }
    let lookup = if entry.actions.iter().any(|spec| spec.name == action) {
        action
    } else {
        bare
    };
    entry
        .actions
        .iter()
        .find(|spec| spec.name == lookup)
        .map(|spec| spec.requires_admin)
        // Unknown actions fail closed. Dispatch will still return its normal
        // unknown-action envelope after an administrator reaches it.
        .unwrap_or(true)
}

#[cfg(test)]
mod tests;
