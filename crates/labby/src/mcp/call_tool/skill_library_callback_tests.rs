use std::sync::Arc;

use axum::http::{Request, header};
use labby_auth::auth_context::AuthContext;
use labby_auth::{Authenticator, VerifiedIdentity};

use super::{skill_library_callback_boundary, skill_library_callback_correlation};
use crate::dispatch::error::ToolError;

fn identity(subject: &str, authenticator: Authenticator) -> VerifiedIdentity {
    VerifiedIdentity::external(authenticator, "https://accounts.google.com", subject)
        .expect("fixture identity")
}

fn parts(
    verified: Option<VerifiedIdentity>,
    via_session: bool,
    scopes: &[&str],
    headers: &[(&str, &str)],
) -> axum::http::request::Parts {
    let mut request = Request::builder().uri("https://lab.example/mcp");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let (mut parts, ()) = request.body(()).expect("request").into_parts();
    parts.extensions.insert(AuthContext {
        sub: "raw-sub-must-not-authorize".to_owned(),
        actor_key: Some(Arc::from("safe-actor")),
        scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
        issuer: "raw-issuer-must-not-authorize".to_owned(),
        via_session,
        csrf_token: Some("server-csrf".to_owned()),
        email: Some("private@example.test".to_owned()),
    });
    if let Some(verified) = verified {
        parts.extensions.insert(verified);
    }
    parts
}

fn assert_forbidden(error: ToolError) {
    assert!(matches!(error, ToolError::Forbidden { .. }));
}

#[test]
fn forged_bridge_metadata_cannot_supply_identity_or_scopes() {
    let parts = parts(None, false, &["lab:admin"], &[]);
    assert_forbidden(
        skill_library_callback_boundary(&parts)
            .err()
            .expect("raw auth metadata is not a verified identity"),
    );
}

#[test]
fn callback_preserves_the_host_verified_identity() {
    let expected = identity("owner", Authenticator::OauthBearer);
    let parts = parts(Some(expected.clone()), false, &["lab"], &[]);
    let boundary = skill_library_callback_boundary(&parts).expect("host callback");
    assert_eq!(boundary.identity, expected);
    assert_eq!(boundary.scopes, ["lab"]);
}

#[test]
fn non_app_text_fallback_needs_no_bridge_metadata() {
    let parts = parts(
        Some(identity("owner", Authenticator::StaticBearer)),
        false,
        &["lab:read"],
        &[],
    );
    skill_library_callback_boundary(&parts).expect("canonical MCP context is sufficient");
}

#[test]
fn cross_origin_cookie_callback_is_denied_even_with_csrf_header() {
    let parts = parts(
        Some(identity("owner", Authenticator::BrowserSession)),
        true,
        &["lab:admin"],
        &[
            (header::COOKIE.as_str(), "labby_session=secret"),
            (header::ORIGIN.as_str(), "https://attacker.example"),
            ("x-csrf-token", "server-csrf"),
        ],
    );
    assert_forbidden(skill_library_callback_boundary(&parts).unwrap_err());
}

#[test]
fn cookie_callback_is_denied_with_missing_or_wrong_csrf() {
    for csrf in [None, Some("wrong-csrf")] {
        let mut headers = vec![(header::COOKIE.as_str(), "labby_session=secret")];
        if let Some(csrf) = csrf {
            headers.push(("x-csrf-token", csrf));
        }
        let parts = parts(
            Some(identity("owner", Authenticator::BrowserSession)),
            true,
            &["lab:admin"],
            &headers,
        );
        assert_forbidden(skill_library_callback_boundary(&parts).unwrap_err());
    }
}

#[test]
fn bearer_and_cookie_ambiguity_is_denied() {
    let parts = parts(
        Some(identity("owner", Authenticator::OauthBearer)),
        false,
        &["lab:admin"],
        &[
            (header::AUTHORIZATION.as_str(), "Bearer redacted"),
            (header::COOKIE.as_str(), "labby_session=redacted"),
        ],
    );
    assert_forbidden(skill_library_callback_boundary(&parts).unwrap_err());
}

#[test]
fn callback_error_does_not_reflect_cookie_or_identity_metadata() {
    let secret = "top-secret-cookie-value";
    let parts = parts(
        None,
        false,
        &["lab:admin"],
        &[(header::COOKIE.as_str(), secret)],
    );
    let error = skill_library_callback_boundary(&parts).unwrap_err();
    let rendered = format!("{error:?}");
    assert!(!rendered.contains(secret));
    assert!(!rendered.contains("private@example.test"));
    assert!(!rendered.contains("raw-sub-must-not-authorize"));
}

#[test]
fn unsafe_correlation_is_rejected_without_reflection() {
    let secret = "secret\nforged-log-field";
    let error = skill_library_callback_correlation(Some(secret)).unwrap_err();
    let rendered = format!("{error:?}");
    assert!(matches!(error, ToolError::InvalidParam { .. }));
    assert!(!rendered.contains(secret));
    assert!(!rendered.contains("forged-log-field"));
}

#[test]
fn stale_app_content_version_cannot_enter_list_params() {
    let stale = serde_json::json!({"content_version": "stale"});
    assert!(
        serde_json::from_value::<crate::dispatch::skill_library::params::PageParams>(stale)
            .is_err()
    );
}

#[test]
fn callback_action_catalog_has_no_app_only_or_stale_aliases() {
    let actions = crate::dispatch::skill_library::catalog::ACTIONS
        .iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>();
    assert_eq!(actions.len(), 13);
    assert!(actions.contains(&"skill_library.list"));
    assert!(!actions.contains(&"open"));
    assert!(!actions.contains(&"skill_library.open"));
}

#[test]
fn protected_route_service_mismatch_denies_the_shared_skills_tool() {
    let denied = crate::mcp::route_scope::McpRouteScope::protected_subset(
        "restricted",
        std::iter::empty::<&str>(),
        ["gateway"],
        false,
    );
    let allowed = crate::mcp::route_scope::McpRouteScope::protected_subset(
        "skills",
        std::iter::empty::<&str>(),
        ["skills"],
        false,
    );
    assert!(!denied.allows_service("skills"));
    assert!(allowed.allows_service("skills"));
}

#[tokio::test]
async fn actual_http_adapter_rejects_hostile_callback_transports_with_safe_correlation() {
    use rmcp::model::{CallToolRequestParams, NumberOrString};
    use rmcp::service::{RequestContext, serve_directly};

    use crate::mcp::logging::{LoggingLevel, logging_level_rank};
    use crate::mcp::server::LabMcpServer;

    let server = LabMcpServer {
        registry: Arc::new(crate::registry::build_default_registry()),
        access_runtime: Arc::new(crate::access::AccessRuntime::blocked_unavailable()),
        #[cfg(feature = "gateway")]
        gateway_manager: None,
        peers: Default::default(),
        code_mode_app_state: Default::default(),
        last_listed_tool_contract: Default::default(),
        route_runtime: Default::default(),
        #[cfg(feature = "gateway")]
        client_registry: Default::default(),
        transport_label: "http",
        logging_level: Arc::new(std::sync::atomic::AtomicU8::new(logging_level_rank(
            LoggingLevel::Emergency,
        ))),
        route_scope: crate::mcp::route_scope::McpRouteScope::Root,
        relay_session_id: 0,
        code_mode_widget_callbacks_enabled_for_test: false,
    };
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running =
        serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(server, transport, None);
    let call = || {
        CallToolRequestParams::new("skills").with_arguments(serde_json::Map::from_iter([
            (
                "action".to_owned(),
                serde_json::Value::String("skill_library.list".to_owned()),
            ),
            ("params".to_owned(), serde_json::json!({})),
        ]))
    };
    let cases = [
        (
            "cross-origin-cookie",
            true,
            Authenticator::BrowserSession,
            vec![
                (header::COOKIE.as_str(), "labby_session=cross-origin-secret"),
                (header::ORIGIN.as_str(), "https://attacker.example"),
                ("x-csrf-token", "server-csrf"),
                ("x-request-id", "safe-cross-origin"),
            ],
            Some("safe-cross-origin"),
        ),
        (
            "missing-csrf",
            true,
            Authenticator::BrowserSession,
            vec![
                (header::COOKIE.as_str(), "labby_session=missing-csrf-secret"),
                ("x-request-id", "safe-missing-csrf"),
            ],
            Some("safe-missing-csrf"),
        ),
        (
            "wrong-csrf",
            true,
            Authenticator::BrowserSession,
            vec![
                (header::COOKIE.as_str(), "labby_session=wrong-csrf-secret"),
                ("x-csrf-token", "wrong-csrf-secret"),
                ("x-request-id", "safe-wrong-csrf"),
            ],
            Some("safe-wrong-csrf"),
        ),
        (
            "bearer-cookie-ambiguity",
            false,
            Authenticator::OauthBearer,
            vec![
                (header::AUTHORIZATION.as_str(), "Bearer bearer-secret"),
                (header::COOKIE.as_str(), "labby_session=ambiguous-secret"),
                ("x-request-id", "../../unsafe-correlation-secret"),
            ],
            None,
        ),
    ];

    for (label, via_session, authenticator, headers, expected_correlation) in cases {
        let mut context = RequestContext::new(NumberOrString::Number(1), running.peer().clone());
        let mut request = Request::builder().uri("https://lab.example/mcp");
        for (name, value) in &headers {
            request = request.header(*name, *value);
        }
        let (mut parts, ()) = request.body(()).expect("hostile request").into_parts();
        parts.headers.insert(
            "x-labby-project-id",
            "bootstrap-default".parse().expect("project header"),
        );
        parts
            .extensions
            .insert(identity("hostile-owner", authenticator));
        parts.extensions.insert(AuthContext {
            sub: "untrusted-raw-sub".to_owned(),
            actor_key: None,
            scopes: vec!["lab:admin".to_owned()],
            issuer: "untrusted-raw-issuer".to_owned(),
            via_session,
            csrf_token: Some("server-csrf".to_owned()),
            email: Some("private@example.test".to_owned()),
        });
        context.extensions.insert(parts);

        let denied = Box::pin(running.service().call_tool_impl(call(), context))
            .await
            .expect("adapter returns a structured denial");
        assert!(denied.is_error.unwrap_or(false), "{label}: {denied:?}");
        let text = denied.content[0]
            .as_text()
            .expect("text error envelope")
            .text
            .as_str();
        let envelope: serde_json::Value =
            serde_json::from_str(text).expect("structured error envelope");
        assert_eq!(envelope["error"]["kind"], "unknown_action", "{label}");
        let correlation = envelope["error"]["correlation_id"]
            .as_str()
            .expect("client-visible correlation");
        if let Some(expected) = expected_correlation {
            assert_eq!(correlation, expected, "{label}");
        } else {
            assert!(correlation.starts_with("mcp-skill-library-rejection-"));
        }
        for secret in [
            "cross-origin-secret",
            "missing-csrf-secret",
            "wrong-csrf-secret",
            "bearer-secret",
            "ambiguous-secret",
            "unsafe-correlation-secret",
            "private@example.test",
            "untrusted-raw-sub",
        ] {
            assert!(!text.contains(secret), "{label} reflected `{secret}`");
        }
    }
}

#[tokio::test]
async fn authenticated_http_call_tool_reaches_process_library_for_read_and_mutation() {
    use std::time::Duration;

    use labby_runtime::artifacts::ArtifactStore;
    use rmcp::model::{CallToolRequestParams, NumberOrString};
    use rmcp::service::{RequestContext, serve_directly};

    use crate::access::{AccessRuntime, AccessStore, BootstrapOwnerInput};
    use crate::dispatch::skill_library::blocking::BoundedBlockingExecutor;
    use crate::dispatch::skill_library::dispatch::{
        ActivationCoordinator, ArtifactFirstPartyProjection, GenerationProjection,
        SkillLibraryService,
    };
    use crate::mcp::logging::{LoggingLevel, logging_level_rank};
    use crate::mcp::server::LabMcpServer;

    let root = tempfile::tempdir().expect("temporary Skill Library root");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private root");
    }
    let owner = identity("mcp-owner", Authenticator::OauthBearer);
    let access_path = root.path().join("access.db");
    let access_store = AccessStore::open(access_path.clone())
        .await
        .expect("access store");
    access_store
        .bootstrap_owner(
            BootstrapOwnerInput::new(owner.clone(), "Local", "Default").expect("owner input"),
        )
        .await
        .expect("bootstrap owner");
    drop(access_store);
    let access_runtime = Arc::new(AccessRuntime::initialize(access_path).await);

    let store =
        Arc::new(ArtifactStore::new(root.path().join("artifacts")).expect("artifact store"));
    let projection: Arc<dyn GenerationProjection<crate::skills::registry::FirstPartyGeneration>> =
        Arc::new(ArtifactFirstPartyProjection);
    let snapshot = store.library_snapshot().expect("initial library snapshot");
    let initial = projection
        .prepare(&store, &snapshot, None)
        .expect("initial generation");
    let service = Arc::new(SkillLibraryService::new(
        store,
        BoundedBlockingExecutor::new(2, Duration::from_secs(1), Duration::from_secs(10))
            .expect("blocking executor"),
        Arc::new(ActivationCoordinator::new(initial, snapshot.version)),
        projection,
    ));
    assert!(
        crate::dispatch::skill_library::install_process_service(service).is_ok(),
        "the production process Skill Library installs once in this regression"
    );

    let server = LabMcpServer {
        registry: Arc::new(crate::registry::build_default_registry()),
        access_runtime,
        #[cfg(feature = "gateway")]
        gateway_manager: None,
        peers: Default::default(),
        code_mode_app_state: Default::default(),
        last_listed_tool_contract: Default::default(),
        route_runtime: Default::default(),
        #[cfg(feature = "gateway")]
        client_registry: Default::default(),
        transport_label: "http",
        logging_level: Arc::new(std::sync::atomic::AtomicU8::new(logging_level_rank(
            LoggingLevel::Emergency,
        ))),
        route_scope: crate::mcp::route_scope::McpRouteScope::Root,
        relay_session_id: 0,
        code_mode_widget_callbacks_enabled_for_test: false,
    };
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running =
        serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(server, transport, None);
    let context = || {
        let mut context = RequestContext::new(NumberOrString::Number(1), running.peer().clone());
        let mut request = Request::builder()
            .uri("https://lab.example/mcp")
            .header("x-labby-project-id", "bootstrap-default")
            .header(header::AUTHORIZATION, "Bearer redacted")
            .body(())
            .expect("request")
            .into_parts()
            .0;
        request.extensions.insert(owner.clone());
        request.extensions.insert(AuthContext {
            sub: "untrusted-raw-sub".to_owned(),
            actor_key: None,
            scopes: vec!["lab:admin".to_owned()],
            issuer: "untrusted-raw-issuer".to_owned(),
            via_session: false,
            csrf_token: None,
            email: None,
        });
        context.extensions.insert(request);
        context
    };
    let call = |action: &str, params: serde_json::Value| {
        CallToolRequestParams::new("skills").with_arguments(serde_json::Map::from_iter([
            (
                "action".to_owned(),
                serde_json::Value::String(action.to_owned()),
            ),
            ("params".to_owned(), params),
        ]))
    };

    let listed = Box::pin(
        running
            .service()
            .call_tool_impl(call("skill_library.list", serde_json::json!({})), context()),
    )
    .await
    .expect("management list response");
    assert!(!listed.is_error.unwrap_or(false), "{listed:?}");

    let created = Box::pin(running.service().call_tool_impl(
        call(
            "skill_library.create",
            serde_json::json!({
                "name": "mcp-production-wire",
                "files": [{
                    "path": "SKILL.md",
                    "content": "---\nname: mcp-production-wire\ndescription: prove MCP production wiring\n---\nbody\n"
                }],
                "expected_library_version": 0,
                "idempotency_key": "mcp-production-wire-create"
            }),
        ),
        context(),
    ))
    .await
    .expect("management mutation response");
    assert!(!created.is_error.unwrap_or(false), "{created:?}");
}

#[cfg(feature = "gateway")]
#[tokio::test]
async fn explicit_mcp_action_allowlist_permits_list_and_denies_create() {
    use rmcp::model::{CallToolRequestParams, NumberOrString};
    use rmcp::service::{RequestContext, serve_directly};

    use crate::mcp::logging::{LoggingLevel, logging_level_rank};
    use crate::mcp::server::LabMcpServer;

    let manager = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager(
            std::path::PathBuf::from("config.toml"),
            crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
        )
        .with_builtin_service_registry(Arc::new(crate::registry::build_default_registry())),
    );
    manager
        .seed_config_unchecked_for_tests(
            crate::config::LabConfig {
                virtual_servers: vec![crate::config::VirtualServerConfig {
                    id: "skills-policy".to_owned(),
                    service: "skills".to_owned(),
                    enabled: true,
                    surfaces: crate::config::VirtualServerSurfacesConfig {
                        mcp: true,
                        ..Default::default()
                    },
                    mcp_policy: Some(crate::config::VirtualServerMcpPolicyConfig {
                        allowed_actions: vec!["skill_library.list".to_owned()],
                    }),
                }],
                ..Default::default()
            }
            .to_gateway_config(),
        )
        .await;
    let server = LabMcpServer {
        registry: Arc::new(crate::registry::build_default_registry()),
        access_runtime: Arc::new(crate::access::AccessRuntime::blocked_unavailable()),
        gateway_manager: Some(manager),
        peers: Default::default(),
        code_mode_app_state: Default::default(),
        last_listed_tool_contract: Default::default(),
        route_runtime: Default::default(),
        client_registry: Default::default(),
        transport_label: "http",
        logging_level: Arc::new(std::sync::atomic::AtomicU8::new(logging_level_rank(
            LoggingLevel::Emergency,
        ))),
        route_scope: crate::mcp::route_scope::McpRouteScope::Root,
        relay_session_id: 0,
        code_mode_widget_callbacks_enabled_for_test: false,
    };
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running =
        serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(server, transport, None);
    let mut context = RequestContext::new(NumberOrString::Number(1), running.peer().clone());
    let mut request = Request::builder()
        .uri("https://lab.example/mcp")
        .header("x-labby-project-id", "bootstrap-default")
        .header(header::AUTHORIZATION, "Bearer redacted")
        .body(())
        .expect("request")
        .into_parts()
        .0;
    request
        .extensions
        .insert(identity("policy-owner", Authenticator::OauthBearer));
    request.extensions.insert(AuthContext {
        sub: "untrusted-sub".to_owned(),
        actor_key: None,
        scopes: vec!["lab:admin".to_owned()],
        issuer: "untrusted-issuer".to_owned(),
        via_session: false,
        csrf_token: None,
        email: None,
    });
    context.extensions.insert(request);

    assert!(
        running
            .service()
            .skill_library_http_action_allowed(&context, "skill_library.list")
            .await
    );
    assert!(
        !running
            .service()
            .skill_library_http_action_allowed(&context, "skill_library.create")
            .await
    );
    let denied = Box::pin(running.service().call_tool_impl(
        CallToolRequestParams::new("skills").with_arguments(serde_json::Map::from_iter([
            (
                "action".to_owned(),
                serde_json::Value::String("skill_library.create".to_owned()),
            ),
            ("params".to_owned(), serde_json::json!({})),
        ])),
        context,
    ))
    .await
    .expect("policy denial response");
    assert!(denied.is_error.unwrap_or(false));
    let text = denied.content[0]
        .as_text()
        .expect("text error")
        .text
        .as_str();
    assert!(text.contains("unknown_action"), "{text}");
}
