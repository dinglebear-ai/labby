//! Unmounted exact regular Prompt execution authorization seam.
#![allow(dead_code)]

use labby_auth::VerifiedIdentity;
use labby_gateway::gateway::manager::{GatewayManager, PublishedPromptCallError};
use rmcp::model::{GetPromptRequestParams, GetPromptResult};
use thiserror::Error;

use crate::access::{AccessRuntime, Permission};
use crate::mcp::bound_access::{BoundAccessContext, bind_asset_use_access_context};

/// Server-owned inputs for one exact regular non-OAuth Prompt execution.
///
/// Deliberately non-`Clone`, non-`Debug`, and non-serializable. Callers must
/// construct it from authenticated identity and protected-route facts, never
/// from MCP params or `_meta` beyond the Prompt request itself.
pub(crate) struct PromptExecutionResolutionInput {
    identity: VerifiedIdentity,
    route_name: String,
    resource: String,
    project_id: String,
    request: GetPromptRequestParams,
}

impl PromptExecutionResolutionInput {
    pub(crate) fn new(
        identity: VerifiedIdentity,
        route_name: impl Into<String>,
        resource: impl Into<String>,
        project_id: impl Into<String>,
        request: GetPromptRequestParams,
    ) -> Self {
        Self {
            identity,
            route_name: route_name.into(),
            resource: resource.into(),
            project_id: project_id.into(),
            request,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum PromptExecutionResolutionError {
    #[error("prompt execution target is unavailable")]
    Unavailable,
    #[error("prompt execution queue is unavailable")]
    QueueUnavailable,
    #[error("prompt execution failed")]
    Upstream,
    #[error("prompt execution timed out")]
    Timeout,
}

struct ExactPromptTarget<'a> {
    upstream: &'a str,
    native_name: &'a str,
    pool_generation: labby_gateway::gateway::manager::PoolPublicationGeneration,
    prompt_generation: labby_gateway::upstream::pool::PromptCatalogGeneration,
}

fn resolve_exact_target<'a>(
    context: &'a BoundAccessContext,
    wire_name: &str,
) -> Option<ExactPromptTarget<'a>> {
    let access = context.catalog().access();
    if access.permission != Permission::AssetUse {
        return None;
    }
    let prompts = context.catalog().catalog().prompts();
    let published = prompts.unique_route_for_wire_name(wire_name)?;
    context
        .allows_upstream_prompt_pair(
            published.upstream_name.as_ref(),
            published.native_name.as_ref(),
        )
        .then_some(ExactPromptTarget {
            upstream: published.upstream_name.as_ref(),
            native_name: published.native_name.as_ref(),
            pool_generation: prompts.pool_publication_generation(),
            prompt_generation: prompts.prompt_catalog_generation(),
        })
}

/// Authorize and execute one exact regular non-OAuth Prompt against a bounded
/// common interval. This remains unmounted from every MCP handler.
pub(crate) async fn execute_exact_project_prompt(
    runtime: &AccessRuntime,
    manager: &GatewayManager,
    input: PromptExecutionResolutionInput,
) -> Result<GetPromptResult, PromptExecutionResolutionError> {
    let wire_name = input.request.name.clone();
    let first = bind_asset_use_access_context(
        runtime,
        manager,
        input.identity.clone(),
        &input.route_name,
        &input.resource,
        &input.project_id,
    )
    .await
    .map_err(|_| PromptExecutionResolutionError::Unavailable)?;
    let target = resolve_exact_target(&first, &wire_name)
        .ok_or(PromptExecutionResolutionError::Unavailable)?;
    let upstream = target.upstream.to_string();
    let native_name = target.native_name.to_string();
    let pool_generation = target.pool_generation;
    let prompt_generation = target.prompt_generation;
    let mut outbound = input.request;
    outbound.name.clone_from(&native_name);
    let result = manager
        .execute_published_prompt_exact(
            pool_generation,
            prompt_generation,
            &upstream,
            &native_name,
            outbound,
        )
        .await;
    let second = bind_asset_use_access_context(
        runtime,
        manager,
        input.identity,
        &input.route_name,
        &input.resource,
        &input.project_id,
    )
    .await
    .map_err(|_| PromptExecutionResolutionError::Unavailable)?;
    if !first.same_publication_as(&second) || resolve_exact_target(&second, &wire_name).is_none() {
        return Err(PromptExecutionResolutionError::Unavailable);
    }
    result.map_err(|error| match error {
        PublishedPromptCallError::Unavailable => PromptExecutionResolutionError::Unavailable,
        PublishedPromptCallError::QueueUnavailable => {
            PromptExecutionResolutionError::QueueUnavailable
        }
        PublishedPromptCallError::Upstream => PromptExecutionResolutionError::Upstream,
        PublishedPromptCallError::Timeout => PromptExecutionResolutionError::Timeout,
    })
}

#[cfg(all(test, feature = "proxy-testkit"))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use labby_auth::{Authenticator, VerifiedIdentity};
    use labby_gateway::gateway::config_store::FsGatewayConfigStore;
    use labby_gateway::gateway::manager::{GatewayManager, GatewayRuntimeHandle};
    use labby_gateway::upstream::pool::UpstreamPool;
    use labby_runtime::gateway_config::{
        GatewayConfig, GatewayLoadoutConfig, ProtectedGatewaySubsetTarget, ProtectedMcpRouteConfig,
        ProtectedMcpRouteTarget, UpstreamConfig,
    };
    use rmcp::model::{
        ErrorData, GetPromptRequestParams, GetPromptResponse, GetPromptResult, Prompt,
        PromptMessage, Role,
    };
    use rmcp::service::RequestContext;
    use rmcp::{RoleServer, ServerHandler};
    use tokio::sync::Notify;

    use super::{
        PromptExecutionResolutionError, PromptExecutionResolutionInput,
        execute_exact_project_prompt,
    };
    use crate::access::{
        AccessRuntime, AssignProjectLoadoutInput, BootstrapOwnerInput, Permission,
        project_runtime_mcp_catalog_context,
    };

    #[derive(Clone)]
    struct EchoPromptServer {
        calls: Arc<AtomicUsize>,
    }

    impl ServerHandler for EchoPromptServer {
        async fn get_prompt(
            &self,
            request: GetPromptRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<GetPromptResponse, ErrorData> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let argument = request
                .arguments
                .as_ref()
                .and_then(|arguments| arguments.get("target"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("missing");
            Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                Role::User,
                format!("{}:{argument}", request.name),
            )])
            .into())
        }
    }

    #[derive(Clone)]
    struct DelayedPromptServer {
        started: Arc<Notify>,
        release: Arc<Notify>,
        fail: bool,
    }

    impl ServerHandler for DelayedPromptServer {
        async fn get_prompt(
            &self,
            request: GetPromptRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<GetPromptResponse, ErrorData> {
            self.started.notify_one();
            self.release.notified().await;
            if self.fail {
                return Err(ErrorData::internal_error("private delayed failure", None));
            }
            Ok(
                GetPromptResult::new(vec![PromptMessage::new_text(Role::User, request.name)])
                    .into(),
            )
        }
    }

    struct Fixture {
        _directory: tempfile::TempDir,
        runtime: Arc<AccessRuntime>,
        gateway_runtime: GatewayRuntimeHandle,
        manager: Arc<GatewayManager>,
        identity: VerifiedIdentity,
        pool: Arc<UpstreamPool>,
        calls: Arc<AtomicUsize>,
    }

    fn gateway_config(expose_prompts: bool) -> GatewayConfig {
        GatewayConfig {
            upstream: ["alpha"]
                .into_iter()
                .map(|name| UpstreamConfig {
                    enabled: true,
                    name: name.into(),
                    url: None,
                    transport: None,
                    socket_path: None,
                    headers: Default::default(),
                    bearer_token_env: None,
                    command: Some("node".into()),
                    args: Vec::new(),
                    env: Default::default(),
                    proxy_resources: false,
                    proxy_prompts: true,
                    expose_tools: None,
                    expose_resources: None,
                    expose_prompts: None,
                    proxy_skills: false,
                    expose_skills: None,
                    code_mode_hint: None,
                    oauth: None,
                    imported_from: None,
                    priority: 1.0,
                })
                .collect(),
            loadouts: vec![GatewayLoadoutConfig {
                name: "production".into(),
                upstreams: vec!["alpha".into()],
                expose_prompts,
                ..GatewayLoadoutConfig::default()
            }],
            protected_mcp_routes: vec![ProtectedMcpRouteConfig {
                name: "project-route".into(),
                enabled: true,
                public_host: "mcp.example.com".into(),
                public_path: "/project".into(),
                upstream: None,
                backend_url: String::new(),
                backend_mcp_path: "/mcp".into(),
                scopes: Vec::new(),
                health_path: None,
                target: Some(ProtectedMcpRouteTarget::GatewaySubset(
                    ProtectedGatewaySubsetTarget {
                        project_id: Some("bootstrap-default".into()),
                        loadout: Some("production".into()),
                        ..ProtectedGatewaySubsetTarget::default()
                    },
                )),
            }],
            ..GatewayConfig::default()
        }
    }

    async fn fixture() -> Fixture {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let runtime = Arc::new(AccessRuntime::initialize(directory.path().join("access.db")).await);
        let identity = VerifiedIdentity::local_credential_with_issuer(
            Authenticator::StaticBearer,
            "server-static-issuer",
            "server-credential",
        )
        .unwrap();
        runtime
            .bootstrap_owner(
                BootstrapOwnerInput::new(identity.clone(), "Local", "Default").unwrap(),
            )
            .await
            .unwrap();
        runtime
            .store()
            .await
            .unwrap()
            .assign_project_loadout(
                AssignProjectLoadoutInput::new(identity.clone(), "bootstrap-default", "production")
                    .unwrap(),
            )
            .await
            .unwrap();

        let calls = Arc::new(AtomicUsize::new(0));
        let pool = Arc::new(UpstreamPool::new());
        pool.install_prompt_server_for_tests(
            "alpha",
            EchoPromptServer {
                calls: Arc::clone(&calls),
            },
        )
        .await;
        pool.insert_prompt_routes_for_tests(
            "alpha",
            vec![Prompt::new("owner/nested/name", Some("exact"), None)],
        )
        .await;
        let gateway_runtime = GatewayRuntimeHandle::default();
        gateway_runtime.swap(Some(Arc::clone(&pool))).await;
        let gateway_path = directory.path().join("prompt-execution.toml");
        let manager = Arc::new(GatewayManager::with_store(
            gateway_path.clone(),
            gateway_runtime.clone(),
            Arc::new(FsGatewayConfigStore::new(gateway_path)),
        ));
        manager.try_seed_config(gateway_config(true)).await.unwrap();
        Fixture {
            _directory: directory,
            runtime,
            gateway_runtime,
            manager,
            identity,
            pool,
            calls,
        }
    }

    fn input(identity: VerifiedIdentity) -> PromptExecutionResolutionInput {
        let arguments = serde_json::Map::from_iter([(
            "target".to_string(),
            serde_json::Value::String("exact-value".to_string()),
        )]);
        PromptExecutionResolutionInput::new(
            identity,
            "project-route",
            "https://mcp.example.com/project",
            "bootstrap-default",
            GetPromptRequestParams::new("alpha/owner/nested/name").with_arguments(arguments),
        )
    }

    #[tokio::test]
    async fn asset_use_executes_exact_native_prompt_and_preserves_arguments() {
        let fixture = fixture().await;
        let result = execute_exact_project_prompt(
            &fixture.runtime,
            &fixture.manager,
            input(fixture.identity.clone()),
        )
        .await
        .expect("owner has AssetUse");

        assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            result.messages,
            vec![PromptMessage::new_text(
                Role::User,
                "owner/nested/name:exact-value"
            )]
        );
    }

    #[tokio::test]
    async fn viewer_and_unknown_wire_target_fail_before_rpc() {
        let fixture = fixture().await;
        fixture
            .runtime
            .store()
            .await
            .unwrap()
            .execute_test_statement(
                "UPDATE project_memberships SET role='viewer' WHERE project_id='bootstrap-default'",
            )
            .await
            .unwrap();
        assert_eq!(
            execute_exact_project_prompt(
                &fixture.runtime,
                &fixture.manager,
                input(fixture.identity.clone()),
            )
            .await,
            Err(PromptExecutionResolutionError::Unavailable)
        );
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);

        fixture
            .runtime
            .store()
            .await
            .unwrap()
            .execute_test_statement(
                "UPDATE project_memberships SET role='owner' WHERE project_id='bootstrap-default'",
            )
            .await
            .unwrap();
        let unknown = PromptExecutionResolutionInput::new(
            fixture.identity,
            "project-route",
            "https://mcp.example.com/project",
            "bootstrap-default",
            GetPromptRequestParams::new("alpha/unknown"),
        );
        assert_eq!(
            execute_exact_project_prompt(&fixture.runtime, &fixture.manager, unknown).await,
            Err(PromptExecutionResolutionError::Unavailable)
        );
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn route_exclusion_fails_before_rpc() {
        let fixture = fixture().await;
        fixture
            .manager
            .try_seed_config(gateway_config(false))
            .await
            .unwrap();

        assert_eq!(
            execute_exact_project_prompt(
                &fixture.runtime,
                &fixture.manager,
                input(fixture.identity),
            )
            .await,
            Err(PromptExecutionResolutionError::Unavailable)
        );
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn pool_publication_aba_discards_delayed_success_and_failure() {
        for fail in [false, true] {
            let fixture = fixture().await;
            let started = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            fixture
                .pool
                .install_prompt_server_for_tests(
                    "alpha",
                    DelayedPromptServer {
                        started: Arc::clone(&started),
                        release: Arc::clone(&release),
                        fail,
                    },
                )
                .await;
            fixture
                .pool
                .insert_prompt_routes_for_tests(
                    "alpha",
                    vec![Prompt::new("owner/nested/name", None::<String>, None)],
                )
                .await;
            fixture
                .pool
                .set_prompt_last_error_for_tests("alpha", Some("replacement sentinel".into()))
                .await;
            let runtime = Arc::clone(&fixture.runtime);
            let manager = Arc::clone(&fixture.manager);
            let request = input(fixture.identity.clone());
            let task = tokio::spawn(async move {
                execute_exact_project_prompt(&runtime, &manager, request).await
            });
            started.notified().await;
            fixture
                .gateway_runtime
                .swap(Some(Arc::new(UpstreamPool::new())))
                .await;
            fixture
                .gateway_runtime
                .swap(Some(Arc::clone(&fixture.pool)))
                .await;
            release.notify_one();
            assert_eq!(
                task.await.unwrap(),
                Err(PromptExecutionResolutionError::Unavailable)
            );
            assert_eq!(
                fixture
                    .pool
                    .prompt_last_error_for_tests("alpha")
                    .await
                    .as_deref(),
                Some("replacement sentinel")
            );
        }
    }

    #[tokio::test]
    async fn access_revocation_during_rpc_discards_result_and_cancellation_returns_no_result() {
        let fixture = fixture().await;
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        fixture
            .pool
            .install_prompt_server_for_tests(
                "alpha",
                DelayedPromptServer {
                    started: Arc::clone(&started),
                    release: Arc::clone(&release),
                    fail: false,
                },
            )
            .await;
        fixture
            .pool
            .insert_prompt_routes_for_tests(
                "alpha",
                vec![Prompt::new("owner/nested/name", None::<String>, None)],
            )
            .await;
        let runtime = Arc::clone(&fixture.runtime);
        let manager = Arc::clone(&fixture.manager);
        let request = input(fixture.identity.clone());
        let task =
            tokio::spawn(
                async move { execute_exact_project_prompt(&runtime, &manager, request).await },
            );
        started.notified().await;
        fixture
            .runtime
            .store()
            .await
            .unwrap()
            .execute_test_statement(
                "UPDATE project_memberships SET role='viewer' WHERE project_id='bootstrap-default'",
            )
            .await
            .unwrap();
        release.notify_one();
        assert_eq!(
            task.await.unwrap(),
            Err(PromptExecutionResolutionError::Unavailable)
        );

        fixture
            .runtime
            .store()
            .await
            .unwrap()
            .execute_test_statement(
                "UPDATE project_memberships SET role='owner' WHERE project_id='bootstrap-default'",
            )
            .await
            .unwrap();
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        fixture
            .pool
            .install_prompt_server_for_tests(
                "alpha",
                DelayedPromptServer {
                    started: Arc::clone(&started),
                    release: Arc::clone(&release),
                    fail: false,
                },
            )
            .await;
        fixture
            .pool
            .insert_prompt_routes_for_tests(
                "alpha",
                vec![Prompt::new("owner/nested/name", None::<String>, None)],
            )
            .await;
        fixture
            .pool
            .set_prompt_last_error_for_tests("alpha", Some("cancellation sentinel".into()))
            .await;
        let runtime = Arc::clone(&fixture.runtime);
        let manager = Arc::clone(&fixture.manager);
        let task = tokio::spawn(async move {
            execute_exact_project_prompt(&runtime, &manager, input(fixture.identity)).await
        });
        started.notified().await;
        let gateway_runtime = fixture.gateway_runtime.clone();
        let swapping = tokio::spawn(async move {
            gateway_runtime
                .swap(Some(Arc::new(UpstreamPool::new())))
                .await;
        });
        swapping.await.unwrap();
        task.abort();
        release.notify_one();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(
            fixture
                .pool
                .prompt_last_error_for_tests("alpha")
                .await
                .as_deref(),
            Some("cancellation sentinel")
        );
    }

    #[tokio::test]
    async fn access_route_and_prompt_aba_each_discard_delayed_result() {
        for mutation in ["access", "route", "prompt"] {
            let fixture = fixture().await;
            let initial_access_revision = project_runtime_mcp_catalog_context(
                &fixture.runtime,
                &fixture.manager,
                fixture.identity.clone(),
                "bootstrap-default",
                Permission::AssetUse,
            )
            .await
            .unwrap()
            .access()
            .global_revision;
            let started = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            fixture
                .pool
                .install_prompt_server_for_tests(
                    "alpha",
                    DelayedPromptServer {
                        started: Arc::clone(&started),
                        release: Arc::clone(&release),
                        fail: false,
                    },
                )
                .await;
            fixture
                .pool
                .insert_prompt_routes_for_tests(
                    "alpha",
                    vec![Prompt::new("owner/nested/name", None::<String>, None)],
                )
                .await;
            let runtime = Arc::clone(&fixture.runtime);
            let manager = Arc::clone(&fixture.manager);
            let request = input(fixture.identity.clone());
            let task = tokio::spawn(async move {
                execute_exact_project_prompt(&runtime, &manager, request).await
            });
            started.notified().await;
            match mutation {
                "access" => {
                    let store = fixture.runtime.store().await.unwrap();
                    store
                        .execute_test_statement(
                            "UPDATE project_memberships SET role='viewer' WHERE project_id='bootstrap-default';
                             UPDATE access_metadata SET global_revision=global_revision+1 WHERE singleton=1",
                        )
                        .await
                        .unwrap();
                    store
                        .execute_test_statement(
                            "UPDATE project_memberships SET role='owner' WHERE project_id='bootstrap-default';
                             UPDATE access_metadata SET global_revision=global_revision+1 WHERE singleton=1",
                        )
                        .await
                        .unwrap();
                    let current_revision = project_runtime_mcp_catalog_context(
                        &fixture.runtime,
                        &fixture.manager,
                        fixture.identity.clone(),
                        "bootstrap-default",
                        Permission::AssetUse,
                    )
                    .await
                    .unwrap()
                    .access()
                    .global_revision;
                    assert_ne!(current_revision, initial_access_revision);
                }
                "route" => {
                    fixture
                        .manager
                        .try_seed_config(gateway_config(false))
                        .await
                        .unwrap();
                    fixture
                        .manager
                        .try_seed_config(gateway_config(true))
                        .await
                        .unwrap();
                }
                "prompt" => {
                    fixture
                        .pool
                        .insert_prompt_routes_for_tests(
                            "alpha",
                            vec![Prompt::new("other", None::<String>, None)],
                        )
                        .await;
                    fixture
                        .pool
                        .insert_prompt_routes_for_tests(
                            "alpha",
                            vec![Prompt::new("owner/nested/name", None::<String>, None)],
                        )
                        .await;
                }
                _ => unreachable!(),
            }
            release.notify_one();
            assert_eq!(
                task.await.unwrap(),
                Err(PromptExecutionResolutionError::Unavailable),
                "{mutation} ABA must not expose the delayed result"
            );
        }
    }
}
