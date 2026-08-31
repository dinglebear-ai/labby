use super::types::RouteDoc;
use crate::app_manifest::{
    APPS_LAUNCHER_ROUTE, APPS_MANIFEST_API_ROUTE, LABBY_APP_HOST_JS_ROUTE,
    SERVER_LOGS_BROWSER_ROUTE, SERVER_LOGS_QUERY_API_ROUTE,
};

pub(crate) const OAUTH_MODE_ONLY: &str = "OAuth mode only";
pub(crate) const BOOTSTRAP_OWNER_RUNTIME_CONDITION: &str = "OAuth mode only; handler requires a browser session, middleware-derived VerifiedIdentity, lab:admin, and the configured admin email; bearer, MCP, CLI, stdio, and loopback identity bypasses are rejected";
pub(crate) const DEV_RUNTIME_CONDITION: &str = "development/mockup routes";
pub(crate) const GATEWAY_RUNTIME_CONDITION: &str =
    "mounted only when the gateway runtime is configured";
pub(crate) const FS_RUNTIME_CONDITION: &str =
    "mounted only when fs is enabled and /v1 auth is configured if LABBY_WEB_UI_AUTH_DISABLED=true";

pub fn build_route_docs(service_names: &[String]) -> Vec<RouteDoc> {
    let mut routes = vec![
        public("GET", "/health", "health", "liveness probe"),
        public(
            "GET",
            "/healthz",
            "oauth_relay",
            "public OAuth callback relay shallow health",
        ),
        public("GET", "/ready", "health", "readiness probe"),
        public(
            "GET",
            "/callback/{machine_id}",
            "oauth_relay",
            "public OAuth callback relay",
        ),
        public(
            "POST",
            "/callback/{machine_id}",
            "oauth_relay",
            "public OAuth callback relay",
        ),
        public(
            "GET",
            "/callback/{machine_id}/{suffix}",
            "oauth_relay",
            "public OAuth callback relay suffix path",
        ),
        public(
            "POST",
            "/callback/{machine_id}/{suffix}",
            "oauth_relay",
            "public OAuth callback relay suffix path",
        ),
        auth(
            "GET",
            "/v1/openapi.json",
            "openapi",
            "OpenAPI JSON document",
        ),
        auth(
            "GET",
            "/v1/docs",
            "openapi",
            "Scalar OpenAPI documentation UI",
        ),
        auth(
            "GET",
            concat!("/v1/", "{service}", "/actions"),
            "services",
            "service action metadata",
        ),
        auth(
            "GET",
            APPS_MANIFEST_API_ROUTE,
            "apps",
            "operator app manifest",
        ),
        auth(
            "GET",
            SERVER_LOGS_QUERY_API_ROUTE,
            "apps",
            "server logs app data query",
        ),
        auth("POST", "/v1/gateway", "gateway", "gateway action dispatch"),
        auth(
            "POST",
            "/v1/gateway/codemode/tools/search",
            "gateway",
            "admin Code Mode tool search",
        ),
        auth(
            "POST",
            "/v1/gateway/codemode/tools/describe",
            "gateway",
            "admin Code Mode tool description",
        ),
        auth(
            "GET",
            "/v1/auth/allowed-emails",
            "auth",
            "list OAuth email allowlist",
        ),
        auth(
            "POST",
            "/v1/auth/allowed-emails",
            "auth",
            "add OAuth email allowlist entry",
        ),
        auth(
            "DELETE",
            "/v1/auth/allowed-emails/{email}",
            "auth",
            "remove OAuth email allowlist entry",
        ),
        RouteDoc {
            runtime_condition: Some(BOOTSTRAP_OWNER_RUNTIME_CONDITION.to_string()),
            ..auth(
                "POST",
                "/v1/access/bootstrap-owner",
                "access",
                "explicit one-time access owner bootstrap; returns only created or already_applied",
            )
        },
        host_validated_auth("POST", "/v1/doctor", "doctor", "doctor action dispatch"),
        relay_admin(
            "GET",
            "/v1/oauth/relay/machines",
            "list public OAuth callback relay machines",
        ),
        relay_admin(
            "POST",
            "/v1/oauth/relay/machines",
            "register public OAuth callback relay machine",
        ),
        relay_admin(
            "GET",
            "/v1/oauth/relay/machines/{machine_id}",
            "get public OAuth callback relay machine",
        ),
        relay_admin(
            "PUT",
            "/v1/oauth/relay/machines/{machine_id}",
            "update public OAuth callback relay machine",
        ),
        relay_admin(
            "DELETE",
            "/v1/oauth/relay/machines/{machine_id}",
            "remove public OAuth callback relay machine",
        ),
        relay_admin(
            "POST",
            "/v1/oauth/relay/machines/{machine_id}/disable",
            "disable public OAuth callback relay machine",
        ),
        relay_admin(
            "POST",
            "/v1/oauth/relay/machines/{machine_id}/enable",
            "enable public OAuth callback relay machine",
        ),
        relay_admin(
            "POST",
            "/v1/oauth/relay/import",
            "import public OAuth callback relay registry",
        ),
        host_validated_auth("POST", "/v1/setup", "setup", "setup action dispatch"),
        gateway_runtime_auth(
            "GET",
            "/v1/gateway/oauth/status",
            "upstream_oauth",
            "upstream OAuth status",
        ),
        gateway_runtime_auth(
            "POST",
            "/v1/gateway/oauth/start",
            "upstream_oauth",
            "start upstream OAuth flow",
        ),
        gateway_runtime_auth(
            "POST",
            "/v1/gateway/oauth/cancel",
            "upstream_oauth",
            "cancel upstream OAuth flow",
        ),
        gateway_runtime_auth(
            "POST",
            "/v1/gateway/oauth/google/revoke",
            "upstream_oauth",
            "revoke the shared Google provider credential",
        ),
        public(
            "GET",
            "/auth/upstream/callback",
            "upstream_oauth",
            "browser callback for upstream OAuth",
        ),
        public(
            "GET",
            "/.well-known/oauth-client",
            "upstream_oauth",
            "upstream OAuth client metadata",
        ),
        public(
            "GET",
            "/gateway/oauth/result",
            "upstream_oauth",
            "browser OAuth completion page",
        ),
        oauth(
            "GET",
            "/.well-known/oauth-authorization-server",
            "oauth metadata",
        ),
        oauth(
            "GET",
            "/.well-known/oauth-protected-resource",
            "OAuth protected-resource metadata",
        ),
        oauth("GET", "/jwks", "OAuth JWKS"),
        oauth("POST", "/register", "OAuth dynamic client registration"),
        oauth("GET", "/authorize", "OAuth authorization endpoint"),
        oauth("POST", "/token", "OAuth token endpoint"),
        bearer_only("POST", "/mcp", "mcp", "MCP streamable HTTP endpoint"),
        bearer_only("GET", "/mcp", "mcp", "MCP streamable HTTP endpoint"),
        browser("GET", "/auth/login", "oauth", "browser login redirect"),
        browser(
            "GET",
            "/auth/session",
            "oauth",
            "browser session introspection",
        ),
        browser("POST", "/auth/logout", "oauth", "browser session logout"),
        browser("GET", APPS_LAUNCHER_ROUTE, "apps", "operator app launcher"),
        browser(
            "GET",
            SERVER_LOGS_BROWSER_ROUTE,
            "apps",
            "server logs app page",
        ),
        public(
            "GET",
            LABBY_APP_HOST_JS_ROUTE,
            "apps",
            "shared app host bridge asset",
        ),
        public(
            "GET",
            "/auth/google/callback",
            "oauth",
            "Google OAuth callback",
        ),
        dev("GET", "/dev/mockup", "development mockup"),
        dev("GET", "/dev/mockup/{name}", "named development mockup"),
    ];

    for service in service_names {
        if !service_has_action_api_route(service) {
            continue;
        }
        let mut route = auth(
            "POST",
            &format!("/v1/{service}"),
            "services",
            "service action dispatch",
        );
        if service == "fs" {
            route.runtime_condition = Some(FS_RUNTIME_CONDITION.to_string());
            route.feature = Some("fs".to_string());
        }
        routes.push(route);
    }

    routes.sort_by(|a, b| {
        (a.path.as_str(), a.method.as_str()).cmp(&(b.path.as_str(), b.method.as_str()))
    });
    routes
}

fn base(method: &str, path: &str, group: &str, notes: &str) -> RouteDoc {
    let session_cookie_allowed = true;
    RouteDoc {
        method: method.to_string(),
        path: path.to_string(),
        surface: "api".to_string(),
        handler_group: group.to_string(),
        feature: None,
        runtime_condition: None,
        auth_required: true,
        bearer_only: false,
        session_cookie_allowed,
        csrf_required: csrf_required(method, session_cookie_allowed),
        host_validation: false,
        master_only: true,
        cache_posture: "not cacheable".to_string(),
        notes: notes.to_string(),
    }
}

fn auth(method: &str, path: &str, group: &str, notes: &str) -> RouteDoc {
    base(method, path, group, notes)
}

fn host_validated_auth(method: &str, path: &str, group: &str, notes: &str) -> RouteDoc {
    RouteDoc {
        host_validation: true,
        ..auth(method, path, group, notes)
    }
}

fn gateway_runtime_auth(method: &str, path: &str, group: &str, notes: &str) -> RouteDoc {
    RouteDoc {
        runtime_condition: Some(GATEWAY_RUNTIME_CONDITION.to_string()),
        ..auth(method, path, group, notes)
    }
}

fn relay_admin(method: &str, path: &str, notes: &str) -> RouteDoc {
    RouteDoc {
        runtime_condition: Some(
            "mounted only when /v1 auth is configured; handler requires lab:admin".to_string(),
        ),
        ..auth(method, path, "oauth_relay", notes)
    }
}

fn bearer_only(method: &str, path: &str, group: &str, notes: &str) -> RouteDoc {
    RouteDoc {
        bearer_only: true,
        session_cookie_allowed: false,
        csrf_required: false,
        ..auth(method, path, group, notes)
    }
}

fn public(method: &str, path: &str, group: &str, notes: &str) -> RouteDoc {
    RouteDoc {
        auth_required: false,
        session_cookie_allowed: false,
        csrf_required: false,
        master_only: false,
        ..base(method, path, group, notes)
    }
}

fn oauth(method: &str, path: &str, notes: &str) -> RouteDoc {
    RouteDoc {
        session_cookie_allowed: false,
        csrf_required: false,
        ..public(method, path, "oauth", notes)
    }
}

fn browser(method: &str, path: &str, group: &str, notes: &str) -> RouteDoc {
    RouteDoc {
        runtime_condition: Some(OAUTH_MODE_ONLY.to_string()),
        auth_required: true,
        session_cookie_allowed: true,
        csrf_required: csrf_required(method, true),
        ..public(method, path, group, notes)
    }
}

fn dev(method: &str, path: &str, notes: &str) -> RouteDoc {
    RouteDoc {
        runtime_condition: Some(DEV_RUNTIME_CONDITION.to_string()),
        auth_required: true,
        session_cookie_allowed: true,
        csrf_required: csrf_required(method, true),
        ..base(method, path, "dev", notes)
    }
}

fn csrf_required(method: &str, session_cookie_allowed: bool) -> bool {
    session_cookie_allowed && !matches!(method, "GET" | "HEAD" | "OPTIONS")
}

pub fn service_has_action_api_route(service: &str) -> bool {
    !matches!(service, "lab_admin" | "doctor" | "setup")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_docs_do_not_include_non_http_service_dispatch_routes() {
        let routes = build_route_docs(&["lab_admin".to_string()]);
        assert!(!routes.iter().any(|route| route.path == "/v1/lab_admin"));
    }

    #[test]
    fn session_mutation_routes_require_csrf() {
        let routes = build_route_docs(&["server_logs".to_string()]);
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
    fn access_bootstrap_documents_the_browser_only_gate() {
        let routes = build_route_docs(&[]);
        let route = routes
            .iter()
            .find(|route| route.method == "POST" && route.path == "/v1/access/bootstrap-owner")
            .unwrap();
        assert!(route.auth_required);
        assert!(route.session_cookie_allowed);
        assert!(route.csrf_required);
        let condition = route.runtime_condition.as_deref().unwrap_or("");
        for expected in [
            "VerifiedIdentity",
            "lab:admin",
            "configured admin email",
            "bearer",
        ] {
            assert!(condition.contains(expected), "missing gate {expected}");
        }
    }

    #[test]
    fn operator_app_routes_are_documented() {
        let routes = build_route_docs(&["server_logs".to_string()]);
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
    fn public_relay_routes_have_expected_auth_docs() {
        let routes = build_route_docs(&[]);
        let callback = routes
            .iter()
            .find(|route| route.method == "GET" && route.path == "/callback/{machine_id}")
            .unwrap();
        assert_eq!(callback.handler_group, "oauth_relay");
        assert!(!callback.auth_required);
        assert!(!callback.session_cookie_allowed);
        assert!(!callback.csrf_required);

        let admin = routes
            .iter()
            .find(|route| route.method == "POST" && route.path == "/v1/oauth/relay/import")
            .unwrap();
        assert_eq!(admin.handler_group, "oauth_relay");
        assert!(admin.auth_required);
        assert!(admin.session_cookie_allowed);
        assert!(admin.csrf_required);
        assert!(
            admin
                .runtime_condition
                .as_deref()
                .unwrap_or("")
                .contains("lab:admin")
        );
    }
}
