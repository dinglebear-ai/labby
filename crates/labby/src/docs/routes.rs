use super::types::RouteDoc;
use crate::api::route_registry::{RouteAuth, build_route_descriptors};

#[cfg(test)]
pub(crate) const OAUTH_MODE_ONLY: &str = "OAuth mode only";
#[cfg(test)]
pub(crate) const DEV_RUNTIME_CONDITION: &str = "development/mockup routes";

pub fn build_route_docs(_service_names: &[String]) -> Vec<RouteDoc> {
    build_route_descriptors()
        .into_iter()
        .map(|route| {
            let session_cookie_allowed =
                matches!(route.auth, RouteAuth::V1 | RouteAuth::BrowserSession);
            let auth_required = matches!(
                route.auth,
                RouteAuth::V1
                    | RouteAuth::BearerOnly
                    | RouteAuth::BrowserSession
                    | RouteAuth::BootstrapProof
            );
            RouteDoc {
                method: route.method.to_string(),
                path: route.path,
                aliases: route.aliases,
                surface: "api".to_string(),
                handler_group: route.mount.to_string(),
                handler_identity: route.handler.to_string(),
                feature: route.feature.map(str::to_string),
                runtime_condition: route.runtime_condition.map(str::to_string),
                auth_required,
                bearer_only: route.auth == RouteAuth::BearerOnly,
                bootstrap_proof: route.auth == RouteAuth::BootstrapProof,
                session_cookie_allowed,
                csrf_required: session_cookie_allowed
                    && !matches!(route.method, "GET" | "HEAD" | "OPTIONS"),
                host_validation: route.host_validation,
                // Browser-session routes are authenticated UI adapters, not
                // master/admin API routes. Keep this axis independent from
                // authentication and CSRF posture.
                master_only: matches!(route.auth, RouteAuth::V1 | RouteAuth::BearerOnly),
                cache_posture: if route.cache_posture != "route-defined" {
                    route.cache_posture
                } else if route.auth == RouteAuth::Public {
                    "route-defined"
                } else {
                    "not cacheable"
                }
                .to_string(),
                failure_disclosure: route.failure_disclosure.to_string(),
                side_effects: route.side_effects.to_string(),
                notes: route.handler.replace('_', " "),
            }
        })
        .collect()
}

pub fn service_has_action_api_route(service: &str) -> bool {
    !matches!(service, "lab_admin" | "doctor" | "setup")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_manifest::{
        APPS_LAUNCHER_ROUTE, APPS_MANIFEST_API_ROUTE, LABBY_APP_HOST_JS_ROUTE,
        SERVER_LOGS_BROWSER_ROUTE, SERVER_LOGS_QUERY_API_ROUTE,
    };

    #[test]
    fn route_docs_do_not_include_non_http_service_dispatch_routes() {
        let routes = build_route_docs(&[]);
        assert!(!routes.iter().any(|route| route.path == "/v1/lab_admin"));
    }

    #[test]
    fn session_mutation_routes_require_csrf() {
        let routes = build_route_docs(&[]);
        let service = routes
            .iter()
            .find(|route| route.method == "POST" && route.path == "/v1/server_logs")
            .unwrap();
        assert!(service.session_cookie_allowed);
        assert!(service.csrf_required);
        let mcp = routes
            .iter()
            .find(|route| route.method == "POST" && route.path == "/mcp")
            .unwrap();
        assert!(mcp.bearer_only);
        assert!(!mcp.session_cookie_allowed);
        assert!(!mcp.csrf_required);
    }

    #[test]
    fn browser_session_auth_is_not_projected_as_master_admin() {
        let routes = build_route_docs(&[]);
        for path in ["/auth/session", APPS_LAUNCHER_ROUTE] {
            let route = routes.iter().find(|route| route.path == path).unwrap();
            assert!(route.auth_required);
            assert!(route.session_cookie_allowed);
            assert!(!route.master_only);
        }
        let logout = routes
            .iter()
            .find(|route| route.path == "/auth/logout")
            .unwrap();
        assert!(logout.csrf_required);
        assert!(!logout.master_only);
        assert_eq!(logout.cache_posture, "not cacheable");
    }

    #[test]
    fn bootstrap_proof_routes_have_distinct_hardened_contract() {
        let routes = build_route_docs(&[]);
        for path in [
            "/auth/bootstrap/consume",
            "/auth/bootstrap/status",
            "/auth/bootstrap/cleanup",
        ] {
            let route = routes.iter().find(|route| route.path == path).unwrap();
            assert!(route.auth_required);
            assert!(route.bootstrap_proof);
            assert!(!route.bearer_only);
            assert!(!route.session_cookie_allowed);
            assert_eq!(route.cache_posture, "private, no-store");
            assert_eq!(route.failure_disclosure, "uniform non-enumerating denial");
        }
        let status = routes
            .iter()
            .find(|route| route.path == "/auth/bootstrap/status")
            .unwrap();
        assert_eq!(status.side_effects, "none_expected");
    }

    #[test]
    fn aliases_are_explicit_runtime_truth() {
        let routes = build_route_docs(&[]);
        let launcher = routes
            .iter()
            .find(|route| route.path == APPS_LAUNCHER_ROUTE)
            .unwrap();
        assert_eq!(launcher.aliases, [format!("{APPS_LAUNCHER_ROUTE}/")]);
        let mockup = routes
            .iter()
            .find(|route| route.path == "/dev/mockup")
            .unwrap();
        assert_eq!(mockup.aliases, ["/dev/mockup/"]);
    }

    #[test]
    fn operator_app_routes_are_documented() {
        let routes = build_route_docs(&[]);
        for (method, path) in [
            ("GET", APPS_MANIFEST_API_ROUTE),
            ("GET", SERVER_LOGS_QUERY_API_ROUTE),
            ("POST", "/v1/server_logs"),
            ("GET", APPS_LAUNCHER_ROUTE),
            ("GET", SERVER_LOGS_BROWSER_ROUTE),
            ("GET", LABBY_APP_HOST_JS_ROUTE),
        ] {
            assert!(
                routes
                    .iter()
                    .any(|route| route.method == method && route.path == path),
                "missing documented route {method} {path}"
            );
        }
    }

    #[test]
    fn descriptor_keys_are_an_exact_set() {
        let descriptor_keys = build_route_descriptors()
            .into_iter()
            .map(|route| (route.method.to_string(), route.path))
            .collect::<std::collections::BTreeSet<_>>();
        let doc_keys = build_route_docs(&[])
            .into_iter()
            .map(|route| (route.method, route.path))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(descriptor_keys, doc_keys);
    }
}
