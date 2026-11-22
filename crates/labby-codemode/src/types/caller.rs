use std::fmt;

/// Caller identity and authorization facts presented to the host-neutral Code Mode kernel.
#[derive(Clone, PartialEq, Eq)]
pub enum CodeModeCaller {
    /// Trusted local caller that is granted all Code Mode capabilities.
    TrustedLocal,
    /// Request-bound product authority captured by the host. The token stays
    /// outside the sandbox and is never propagated to an MCP upstream.
    WithAuthority {
        caller: Box<CodeModeCaller>,
        authority_token: String,
    },
    /// Authenticated/scoped caller with host-computed capability booleans.
    Scoped {
        /// Capabilities granted by the host surface.
        capabilities: CodeModeCallerCapabilities,
        /// JWT `sub` claim for the caller, when available. The host decides how
        /// to map this onto its own credential/identity model when resolving
        /// and calling tools; the kernel itself never interprets it.
        sub: Option<String>,
        host_context: CodeModeHostContext,
    },
}

/// Request-bound credentials held exclusively by the host. Only an explicitly
/// supplied private-hop token may be forwarded to an in-process upstream.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct CodeModeHostContext {
    pub upstream_private_token: Option<String>,
    pub skill_context_token: Option<String>,
    pub provider: Option<CodeModeProviderContext>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct CodeModeProviderContext {
    pub token: String,
    pub request_id: String,
}

impl fmt::Debug for CodeModeProviderContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CodeModeProviderContext")
            .field("token", &"[REDACTED]")
            .field("request_id", &self.request_id)
            .finish()
    }
}

impl fmt::Debug for CodeModeHostContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CodeModeHostContext")
            .field(
                "upstream_private_token",
                &self.upstream_private_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "skill_context_token",
                &self.skill_context_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("provider", &self.provider)
            .finish()
    }
}

impl fmt::Debug for CodeModeCaller {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TrustedLocal => f.write_str("TrustedLocal"),
            Self::WithAuthority { caller, .. } => f
                .debug_struct("WithAuthority")
                .field("caller", caller)
                .field("authority_token", &"[REDACTED]")
                .finish(),
            Self::Scoped {
                capabilities,
                sub,
                host_context,
            } => f
                .debug_struct("Scoped")
                .field("capabilities", capabilities)
                .field("sub", sub)
                .field("host_context", host_context)
                .finish(),
        }
    }
}

/// Host-computed authorization facts for a scoped Code Mode caller.
///
/// The Code Mode kernel stays independent of Lab's OAuth scope names; surface
/// adapters translate their own auth model into these booleans before calling
/// into the kernel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CodeModeCallerCapabilities {
    /// Whether read-only Code Mode execution is allowed.
    pub can_read: bool,
    /// Whether full Code Mode execution is allowed.
    pub can_execute: bool,
    /// Whether reusable snippets may be listed/resolved/executed.
    pub can_use_snippets: bool,
    /// Whether the caller carries administrative scope.
    pub is_admin: bool,
}

/// Product surface that initiated a Code Mode execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeModeSurface {
    /// Model Context Protocol surface.
    Mcp,
    /// Local command-line interface.
    Cli,
    Api,
}

impl CodeModeSurface {
    /// Stable lowercase surface tag (`"mcp"`, `"cli"`, or `"api"`) used by hosts when
    /// building their own runtime-owner / logging context.
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            CodeModeSurface::Mcp => "mcp",
            CodeModeSurface::Cli => "cli",
            CodeModeSurface::Api => "api",
        }
    }
}

/// Whether a destructive tool call is permitted for this caller.
/// Code Mode execution is already scope-gated; do not add a second host-side
/// confirmation gate based on tool catalog metadata. Hosts call this when
/// applying destructive-tool policy.
#[must_use]
pub fn destructive_permitted(surface: CodeModeSurface, caller: &CodeModeCaller) -> bool {
    match surface {
        CodeModeSurface::Cli => true,
        CodeModeSurface::Mcp | CodeModeSurface::Api => caller.can_execute(),
    }
}

impl CodeModeCaller {
    /// Product-host request context token, never exposed to sandbox code.
    #[must_use]
    pub fn authority_token(&self) -> Option<&str> {
        match self {
            Self::WithAuthority {
                authority_token, ..
            } => Some(authority_token),
            _ => None,
        }
    }

    /// Inner caller after removing the host-only authority wrapper.
    #[must_use]
    pub fn without_authority(&self) -> &Self {
        match self {
            Self::WithAuthority { caller, .. } => caller.without_authority(),
            _ => self,
        }
    }
    fn capabilities(&self) -> CodeModeCallerCapabilities {
        match self {
            Self::WithAuthority { caller, .. } => caller.capabilities(),
            Self::TrustedLocal => CodeModeCallerCapabilities {
                can_read: true,
                can_execute: true,
                can_use_snippets: true,
                is_admin: true,
            },
            Self::Scoped { capabilities, .. } => *capabilities,
        }
    }

    /// Return whether the caller may use reusable Code Mode snippets.
    #[must_use]
    pub fn can_use_snippets(&self) -> bool {
        self.capabilities().can_use_snippets
    }

    /// Return whether the caller may execute full Code Mode workloads.
    #[must_use]
    pub fn can_execute(&self) -> bool {
        self.capabilities().can_execute
    }

    /// Return whether the caller may use the read-only Code Mode surface.
    #[must_use]
    pub fn can_read(&self) -> bool {
        self.capabilities().can_read
    }

    /// Whether this caller carries the `lab:admin` scope (trusted-local always
    /// counts as admin). Hosts use this when mapping the caller onto their own
    /// credential model.
    #[must_use]
    pub fn is_admin(&self) -> bool {
        self.capabilities().is_admin
    }

    /// The caller's `sub` identity, when available. `None` for trusted-local.
    #[must_use]
    pub fn subject(&self) -> Option<&str> {
        match self {
            Self::WithAuthority { caller, .. } => caller.subject(),
            Self::TrustedLocal => None,
            Self::Scoped { sub, .. } => sub.as_deref(),
        }
    }

    /// Request credentials remain outside the runner protocol.
    #[must_use]
    pub fn host_context(&self) -> Option<&CodeModeHostContext> {
        match self.without_authority() {
            Self::Scoped { host_context, .. } => Some(host_context),
            _ => None,
        }
    }

    #[must_use]
    pub fn host_provider_token(&self) -> Option<&str> {
        self.host_context()?
            .provider
            .as_ref()
            .map(|p| p.token.as_str())
    }

    #[must_use]
    pub fn host_provider_request_id(&self) -> Option<&str> {
        self.host_context()?
            .provider
            .as_ref()
            .map(|p| p.request_id.as_str())
    }
}
