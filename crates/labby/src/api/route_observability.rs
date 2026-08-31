//! Redacted, response-side observability for successfully matched API routes.

use std::{collections::BTreeSet, sync::Arc};

use axum::{
    extract::{MatchedPath, Request, State},
    middleware::Next,
    response::Response,
};

use super::route_registry::RouteDescriptor;

#[derive(Clone)]
pub(crate) struct RouteObservability {
    descriptors: Arc<[RouteDescriptor]>,
    mounted: Arc<BTreeSet<(String, String)>>,
}

#[derive(Clone)]
pub(crate) struct RuntimeMatchedRoute(pub(crate) &'static str);

fn route_match_kind(
    actually_matched: bool,
    runtime_conditional: bool,
    status: axum::http::StatusCode,
) -> &'static str {
    if actually_matched {
        "mounted_route_match"
    } else if runtime_conditional && status == axum::http::StatusCode::NOT_FOUND {
        "declared_conditional_absence"
    } else {
        "declared_unexpected_outcome"
    }
}

impl RouteObservability {
    pub(crate) fn new(mounted: &[RouteDescriptor], declared: Vec<RouteDescriptor>) -> Self {
        Self {
            descriptors: declared.into(),
            mounted: Arc::new(
                mounted
                    .iter()
                    .map(|route| (route.method.to_owned(), route.path.clone()))
                    .collect(),
            ),
        }
    }

    fn descriptor(&self, method: &str, template: &str) -> Option<&RouteDescriptor> {
        self.descriptors.iter().find(|descriptor| {
            descriptor.method == method
                && (descriptor.path == template
                    || descriptor.aliases.iter().any(|alias| alias == template))
        })
    }

    fn descriptor_for_request(
        &self,
        method: &str,
        path: &str,
        host: Option<&str>,
    ) -> Option<&RouteDescriptor> {
        self.descriptor(method, path)
            .or_else(|| {
                self.descriptors.iter().find(|descriptor| {
                    descriptor.method == method
                        && !descriptor.path.contains("runtime_protected_mcp_route")
                        && path_matches_template(&descriptor.path, path)
                })
            })
            .or_else(|| {
                self.descriptors.iter().find(|descriptor| {
                    descriptor.method == method
                        && descriptor.path.contains("runtime_protected_mcp_route")
                        && host.is_some_and(|value| !is_loopback_authority(value))
                        && path_matches_template(&descriptor.path, path)
                })
            })
    }
}

fn is_loopback_authority(value: &str) -> bool {
    let host = value
        .strip_prefix('[')
        .and_then(|value| value.split_once(']').map(|(host, _)| host))
        .unwrap_or_else(|| value.split(':').next().unwrap_or(value));
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

fn path_matches_template(template: &str, path: &str) -> bool {
    let template_segments = template.trim_matches('/').split('/').collect::<Vec<_>>();
    let path_segments = path.trim_matches('/').split('/').collect::<Vec<_>>();
    let mut path_index = 0;
    for segment in template_segments {
        if segment.starts_with("{*") {
            return path_index < path_segments.len();
        }
        let Some(actual) = path_segments.get(path_index) else {
            return false;
        };
        if !(segment.starts_with('{') && segment.ends_with('}')) && segment != *actual {
            return false;
        }
        path_index += 1;
    }
    path_index == path_segments.len()
}

pub(crate) async fn record_matched_route(
    State(registry): State<RouteObservability>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method().as_str().to_owned();
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let path = request.uri().path().to_owned();
    let host = request
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok());
    let descriptor = registry
        .descriptor_for_request(&method, &path, host)
        .map(|route| {
            (
                route.path.clone(),
                route.mount,
                route.handler,
                route.runtime_condition,
                registry
                    .mounted
                    .contains(&(method.clone(), route.path.clone())),
            )
        });
    let matched_path = request
        .extensions()
        .get::<MatchedPath>()
        .map(|matched| matched.as_str().to_owned());
    let response = next.run(request).await;
    if let Some((template, route_group, handler, runtime_condition, mounted)) = descriptor.as_ref()
    {
        let runtime_match = response
            .extensions()
            .get::<RuntimeMatchedRoute>()
            .is_some_and(|matched| matched.0 == template);
        let actually_matched = runtime_match
            || (*mounted
                && matched_path.as_deref().is_some_and(|matched| {
                    matched == template
                        || registry
                            .descriptor(&method, matched)
                            .is_some_and(|matched_descriptor| matched_descriptor.path == *template)
                }));
        let match_kind = route_match_kind(
            actually_matched,
            runtime_condition.is_some(),
            response.status(),
        );
        tracing::info!(
            surface = "api",
            http_route_evidence = true,
            request_id = request_id.as_deref().unwrap_or("-"),
            method,
            matched_route = template,
            route_group,
            handler,
            route_match_kind = match_kind,
            runtime_condition = runtime_condition.unwrap_or("-"),
            status = response.status().as_u16(),
            "HTTP route completed"
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::route_registry::RouteAuth;

    #[test]
    fn resolves_aliases_without_observing_concrete_resource_values() {
        let declared = vec![
            RouteDescriptor::new(
                "GET",
                "/v1/things/{id}",
                "show_thing",
                "things",
                RouteAuth::V1,
            )
            .aliases(&["/v1/items/{id}"]),
        ];
        let registry = RouteObservability::new(&declared, declared.clone());
        let route = registry
            .descriptor("GET", "/v1/items/{id}")
            .expect("alias descriptor");
        assert_eq!(route.handler, "show_thing");
        assert!(
            registry
                .descriptor("GET", "/v1/items/secret-value")
                .is_none()
        );
        assert_eq!(
            route_match_kind(false, true, axum::http::StatusCode::UNAUTHORIZED),
            "declared_unexpected_outcome"
        );
        assert_eq!(
            route_match_kind(false, true, axum::http::StatusCode::NOT_FOUND),
            "declared_conditional_absence"
        );
    }

    #[test]
    fn resolves_parameterized_and_outer_protected_routes_before_authentication() {
        let declared = vec![
            RouteDescriptor::new("GET", "/v1/things/{id}", "show", "things", RouteAuth::V1),
            RouteDescriptor::new(
                "POST",
                "/{runtime_protected_mcp_route}",
                "protected",
                "protected_mcp",
                RouteAuth::BearerOnly,
            ),
        ];
        let registry = RouteObservability::new(&declared, declared.clone());
        assert_eq!(
            registry
                .descriptor_for_request("GET", "/v1/things/opaque", Some("127.0.0.1"))
                .map(|route| route.path.as_str()),
            Some("/v1/things/{id}")
        );
        assert_eq!(
            registry
                .descriptor_for_request("POST", "/operator", Some("mcp.example.test"))
                .map(|route| route.path.as_str()),
            Some("/{runtime_protected_mcp_route}")
        );
        assert!(
            registry
                .descriptor_for_request("POST", "/unknown", Some("127.0.0.1"))
                .is_none()
        );
    }

    #[test]
    fn non_not_found_response_cannot_promote_an_unmounted_declaration() {
        let declared = vec![
            RouteDescriptor::new("GET", "/conditional", "conditional", "test", RouteAuth::V1)
                .when("feature"),
        ];
        let registry = RouteObservability::new(&[], declared);
        let descriptor = registry
            .descriptor_for_request("GET", "/conditional", None)
            .unwrap();
        assert!(
            !registry
                .mounted
                .contains(&("GET".to_string(), descriptor.path.clone()))
        );
    }
}
