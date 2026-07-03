use std::collections::BTreeSet;

use crate::config::{ProtectedGatewaySubsetTarget, ProtectedMcpRouteConfig};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum McpRouteScope {
    #[default]
    Root,
    ProtectedSubset {
        route_name: String,
        upstreams: BTreeSet<String>,
        services: BTreeSet<String>,
        expose_code_mode: bool,
    },
}

/// The identity a route-scope label encodes: exactly `root` vs
/// `protected:<route_name>`, and nothing else (the allowed upstream/service sets
/// are runtime policy, not part of the stored durable-run scope label).
///
/// This is the parsed counterpart of [`McpRouteScope::label`]: the durable pause
/// store persists the scope as a `String`, and the cross-route resume/reject
/// guard compares the stored label against the live route by parsing BOTH into a
/// `RouteScopeIdentity`. An unparseable stored label yields `None`, so the guard
/// fails closed (refuses the resume) rather than string-matching a scope it can
/// no longer interpret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RouteScopeIdentity {
    Root,
    Protected { route_name: String },
}

impl RouteScopeIdentity {
    /// Parse a stored route-scope label (`"root"` or `"protected:<name>"`).
    /// Returns `None` for any other shape (fail closed). Paired with
    /// [`McpRouteScope::label`] so `from_label(scope.label())` round-trips a
    /// scope's identity.
    pub(crate) fn from_label(label: &str) -> Option<Self> {
        if label == "root" {
            return Some(Self::Root);
        }
        let route_name = label.strip_prefix("protected:")?;
        if route_name.is_empty() {
            return None;
        }
        Some(Self::Protected {
            route_name: route_name.to_string(),
        })
    }
}

impl McpRouteScope {
    pub(crate) fn protected_subset<I, J, S, T>(
        route_name: impl Into<String>,
        upstreams: I,
        services: J,
        expose_code_mode: bool,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        J: IntoIterator<Item = T>,
        S: AsRef<str>,
        T: AsRef<str>,
    {
        Self::ProtectedSubset {
            route_name: route_name.into(),
            upstreams: upstreams
                .into_iter()
                .map(|name| name.as_ref().to_string())
                .collect(),
            services: services
                .into_iter()
                .map(|name| name.as_ref().to_string())
                .collect(),
            expose_code_mode,
        }
    }

    pub(crate) fn from_protected_route(route: &ProtectedMcpRouteConfig) -> Option<Self> {
        let target: &ProtectedGatewaySubsetTarget = route.gateway_subset_target()?;
        Some(Self::protected_subset(
            route.name.clone(),
            target.upstreams.iter().map(String::as_str),
            target.services.iter().map(String::as_str),
            target.expose_code_mode,
        ))
    }

    pub(crate) fn label(&self) -> String {
        match self {
            Self::Root => "root".to_string(),
            Self::ProtectedSubset { route_name, .. } => format!("protected:{route_name}"),
        }
    }

    /// The scope identity this route carries, for comparing against a stored
    /// durable-run route-scope label. This is the parsed counterpart of
    /// [`Self::label`]: `RouteScopeIdentity::from_label(scope.label())` equals
    /// `scope.identity()`.
    pub(crate) fn identity(&self) -> RouteScopeIdentity {
        match self {
            Self::Root => RouteScopeIdentity::Root,
            Self::ProtectedSubset { route_name, .. } => RouteScopeIdentity::Protected {
                route_name: route_name.clone(),
            },
        }
    }

    pub(crate) fn protected_history_label(&self) -> Option<String> {
        match self {
            Self::Root => None,
            Self::ProtectedSubset { .. } => Some(self.label()),
        }
    }

    pub(crate) fn allows_service(&self, service: &str) -> bool {
        match self {
            Self::Root => true,
            Self::ProtectedSubset { services, .. } => services.contains(service),
        }
    }

    pub(crate) fn allows_upstream(&self, upstream: &str) -> bool {
        match self {
            Self::Root => true,
            Self::ProtectedSubset { upstreams, .. } => upstreams.contains(upstream),
        }
    }

    pub(crate) fn exposes_code_mode(&self) -> bool {
        match self {
            Self::Root => true,
            Self::ProtectedSubset {
                expose_code_mode, ..
            } => *expose_code_mode,
        }
    }

    pub(crate) fn allowed_upstreams(&self) -> Option<&BTreeSet<String>> {
        match self {
            Self::Root => None,
            Self::ProtectedSubset { upstreams, .. } => Some(upstreams),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_allows_everything() {
        let scope = McpRouteScope::Root;
        assert!(scope.allows_service("gateway"));
        assert!(scope.allows_upstream("sonarr"));
        assert!(scope.exposes_code_mode());
        assert_eq!(scope.label(), "root");
    }

    #[test]
    fn protected_subset_allows_only_configured_names() {
        let scope =
            McpRouteScope::protected_subset("media", ["sonarr", "radarr"], ["gateway"], true);
        assert!(scope.allows_service("gateway"));
        assert!(!scope.allows_service("logs"));
        assert!(scope.allows_upstream("sonarr"));
        assert!(!scope.allows_upstream("github"));
        assert!(scope.exposes_code_mode());
        assert_eq!(scope.label(), "protected:media");
    }

    #[test]
    fn protected_subset_can_hide_code_mode() {
        let scope = McpRouteScope::protected_subset("ops", ["unifi"], ["device"], false);
        assert!(!scope.exposes_code_mode());
    }

    #[test]
    fn identity_label_round_trips_for_each_variant() {
        // The cross-route resume/reject guard relies on
        // `RouteScopeIdentity::from_label(scope.label()) == scope.identity()`
        // for every variant, so a run's stored label parses back to the same
        // identity it was written under.
        for scope in [
            McpRouteScope::Root,
            McpRouteScope::protected_subset("media", ["sonarr"], ["gateway"], true),
            McpRouteScope::protected_subset("ops", ["unifi"], ["device"], false),
        ] {
            assert_eq!(
                RouteScopeIdentity::from_label(&scope.label()),
                Some(scope.identity()),
                "label {:?} must parse back to the same identity",
                scope.label()
            );
        }
    }

    #[test]
    fn from_label_rejects_unparseable_scopes_fail_closed() {
        // An unparseable stored label yields None so the guard fails closed.
        assert_eq!(RouteScopeIdentity::from_label("bogus"), None);
        assert_eq!(RouteScopeIdentity::from_label("protected:"), None);
        assert_eq!(RouteScopeIdentity::from_label(""), None);
        // A distinct protected route does not compare equal to another.
        assert_ne!(
            RouteScopeIdentity::from_label("protected:a"),
            RouteScopeIdentity::from_label("protected:b"),
        );
    }
}
