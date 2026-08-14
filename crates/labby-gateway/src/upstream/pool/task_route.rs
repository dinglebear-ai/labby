//! Surface-neutral authorization snapshot attached to retained MCP task routes.

use std::collections::BTreeSet;

/// Authorization boundary captured when an upstream task handle is minted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskRouteAuthorization {
    pub(super) route_key: String,
    pub(super) allowed_upstreams: Option<BTreeSet<String>>,
}

impl TaskRouteAuthorization {
    pub fn new(route_key: impl Into<String>, allowed_upstreams: Option<BTreeSet<String>>) -> Self {
        Self {
            route_key: route_key.into(),
            allowed_upstreams,
        }
    }

    pub fn root() -> Self {
        Self::new("root", None)
    }
}
