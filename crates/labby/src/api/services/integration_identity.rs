//! Authenticated adapter for the immutable daemon-owned identity snapshot.

use crate::api::route_registry::{RouteAuth, RouteDescriptor, RouteGroup};
use crate::integration_identity::IntegrationIdentity;
use axum::{Json, routing::get};

pub(crate) fn descriptors() -> Vec<RouteDescriptor> {
    vec![RouteDescriptor::new("GET", "/identity", "integration_identity", "integration", RouteAuth::V1)
        .when("mounted only with conventional API credentials, initialized installation identity, and no trusted-host integration")
        .private_no_store()]
}

pub(crate) fn routes(snapshot: IntegrationIdentity) -> RouteGroup {
    RouteGroup::empty().route(
        descriptors().remove(0),
        get(move || {
            let snapshot = snapshot.clone();
            async move {
                (
                    [(axum::http::header::CACHE_CONTROL, "private, no-store")],
                    Json(snapshot),
                )
            }
        }),
    )
}
