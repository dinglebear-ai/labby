//! Unmounted exact regular Resource read authorization seam.
#![allow(dead_code)]

use labby_auth::VerifiedIdentity;
use labby_gateway::gateway::manager::{GatewayManager, PublishedResourceReadError};
use rmcp::model::{ReadResourceRequestParams, ReadResourceResult};
use thiserror::Error;

use crate::access::{AccessRuntime, Permission};
use crate::mcp::bound_access::{BoundAccessContext, bind_asset_use_access_context};

/// Server-owned inputs for one exact regular non-OAuth Resource read.
///
/// Deliberately non-`Clone`, non-`Debug`, and non-serializable. The identity
/// and protected-route facts must be trusted server inputs. This unmounted seam
/// does not itself prove a transport token instance or expiry.
pub(crate) struct ResourceReadResolutionInput {
    identity: VerifiedIdentity,
    route_name: String,
    resource: String,
    project_id: String,
    request: ReadResourceRequestParams,
}

impl ResourceReadResolutionInput {
    pub(crate) fn new(
        identity: VerifiedIdentity,
        route_name: impl Into<String>,
        resource: impl Into<String>,
        project_id: impl Into<String>,
        request: ReadResourceRequestParams,
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
pub(crate) enum ResourceReadResolutionError {
    #[error("resource read target is unavailable")]
    Unavailable,
    #[error("resource read queue is unavailable")]
    QueueUnavailable,
    #[error("resource read failed")]
    Upstream,
    #[error("resource read timed out")]
    Timeout,
    #[error("resource response is too large")]
    TooLarge,
}

struct ExactResourceTarget<'a> {
    upstream: &'a str,
    native_uri: &'a str,
    pool_generation: labby_gateway::gateway::manager::PoolPublicationGeneration,
    resource_generation: labby_gateway::upstream::pool::ResourceCatalogGeneration,
}

fn resolve_exact_target<'a>(
    context: &'a BoundAccessContext,
    wire_uri: &str,
) -> Option<ExactResourceTarget<'a>> {
    if context.catalog().access().permission != Permission::AssetUse {
        return None;
    }
    let resources = context.catalog().catalog().resources();
    let published = resources.unique_route_for_wire_uri(wire_uri)?;
    context
        .allows_upstream_resource_pair(
            published.upstream_name.as_ref(),
            published.native_uri.as_ref(),
        )
        .then_some(ExactResourceTarget {
            upstream: published.upstream_name.as_ref(),
            native_uri: published.native_uri.as_ref(),
            pool_generation: resources.pool_publication_generation(),
            resource_generation: resources.resource_catalog_generation(),
        })
}

/// Authorize and read one exact regular non-OAuth Resource against a bounded
/// Access/manager common interval. This remains unmounted from MCP handlers.
pub(crate) async fn read_exact_project_resource(
    runtime: &AccessRuntime,
    manager: &GatewayManager,
    input: ResourceReadResolutionInput,
) -> Result<ReadResourceResult, ResourceReadResolutionError> {
    let wire_uri = input.request.uri.clone();
    let first = bind_asset_use_access_context(
        runtime,
        manager,
        input.identity.clone(),
        &input.route_name,
        &input.resource,
        &input.project_id,
    )
    .await
    .map_err(|_| ResourceReadResolutionError::Unavailable)?;
    let target =
        resolve_exact_target(&first, &wire_uri).ok_or(ResourceReadResolutionError::Unavailable)?;
    let upstream = target.upstream.to_string();
    let native_uri = target.native_uri.to_string();
    let pool_generation = target.pool_generation;
    let resource_generation = target.resource_generation;
    let mut outbound = input.request;
    outbound.uri.clone_from(&native_uri);
    let result = manager
        .execute_published_resource_exact(
            pool_generation,
            resource_generation,
            &upstream,
            &native_uri,
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
    .map_err(|_| ResourceReadResolutionError::Unavailable)?;
    let Some(second_target) = resolve_exact_target(&second, &wire_uri) else {
        return Err(ResourceReadResolutionError::Unavailable);
    };
    if !first.same_publication_as(&second)
        || second_target.upstream != upstream
        || second_target.native_uri != native_uri
        || second_target.pool_generation != pool_generation
        || second_target.resource_generation != resource_generation
    {
        return Err(ResourceReadResolutionError::Unavailable);
    }
    result.map_err(|error| match error {
        PublishedResourceReadError::Unavailable => ResourceReadResolutionError::Unavailable,
        PublishedResourceReadError::QueueUnavailable => {
            ResourceReadResolutionError::QueueUnavailable
        }
        PublishedResourceReadError::Upstream => ResourceReadResolutionError::Upstream,
        PublishedResourceReadError::Timeout => ResourceReadResolutionError::Timeout,
        PublishedResourceReadError::TooLarge => ResourceReadResolutionError::TooLarge,
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
        ErrorData, ListResourcesResult, PaginatedRequestParams, ReadResourceRequestParams,
        ReadResourceResponse, ReadResourceResult, RequestMetaObject, Resource, ResourceContents,
    };
    use rmcp::service::RequestContext;
    use rmcp::{RoleServer, ServerHandler};
    use tokio::sync::{Mutex, Notify};

    use super::{
        ResourceReadResolutionError, ResourceReadResolutionInput, read_exact_project_resource,
    };
    use crate::access::{
        AccessRuntime, AssignProjectLoadoutInput, BootstrapOwnerInput, Permission,
        project_runtime_mcp_catalog_context,
    };

    #[derive(Clone)]
    struct EchoResourceServer {
        calls: Arc<AtomicUsize>,
        received: Arc<Mutex<Vec<(ReadResourceRequestParams, RequestMetaObject)>>>,
    }

    impl ServerHandler for EchoResourceServer {
        async fn list_resources(
            &self,
            _: Option<PaginatedRequestParams>,
            _: RequestContext<RoleServer>,
        ) -> Result<ListResourcesResult, ErrorData> {
            Ok(ListResourcesResult::with_all_items(vec![Resource::new(
                "lab://upstream/inner/file:///nested/value",
                "exact",
            )]))
        }

        async fn read_resource(
            &self,
            request: ReadResourceRequestParams,
            context: RequestContext<RoleServer>,
        ) -> Result<ReadResourceResponse, ErrorData> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.received
                .lock()
                .await
                .push((request.clone(), context.meta.clone()));
            Ok(ReadResourceResult::new(vec![
                ResourceContents::text("exact", "https://wrong.invalid/text"),
                ResourceContents::blob("YWJj", "https://wrong.invalid/blob"),
            ])
            .into())
        }
    }

    #[derive(Clone)]
    struct DelayedResourceServer {
        started: Arc<Notify>,
        release: Arc<Notify>,
        fail: bool,
    }

    impl ServerHandler for DelayedResourceServer {
        async fn read_resource(
            &self,
            request: ReadResourceRequestParams,
            _: RequestContext<RoleServer>,
        ) -> Result<ReadResourceResponse, ErrorData> {
            self.started.notify_one();
            self.release.notified().await;
            if self.fail {
                return Err(ErrorData::internal_error(
                    "private delayed resource failure",
                    None,
                ));
            }
            Ok(
                ReadResourceResult::new(vec![ResourceContents::text("delayed", request.uri)])
                    .into(),
            )
        }
    }

    #[derive(Clone)]
    struct FailingResourceServer;

    impl ServerHandler for FailingResourceServer {
        async fn read_resource(
            &self,
            _: ReadResourceRequestParams,
            _: RequestContext<RoleServer>,
        ) -> Result<ReadResourceResponse, ErrorData> {
            Err(ErrorData::invalid_params(
                "private stable resource failure",
                None,
            ))
        }
    }

    #[derive(Clone)]
    struct OversizedResourceServer;

    impl ServerHandler for OversizedResourceServer {
        async fn read_resource(
            &self,
            request: ReadResourceRequestParams,
            _: RequestContext<RoleServer>,
        ) -> Result<ReadResourceResponse, ErrorData> {
            Ok(ReadResourceResult::new(vec![ResourceContents::text(
                "x".repeat(12 * 1024 * 1024),
                request.uri,
            )])
            .into())
        }
    }

    struct Fixture {
        _directory: tempfile::TempDir,
        runtime: Arc<AccessRuntime>,
        manager: Arc<GatewayManager>,
        gateway_runtime: GatewayRuntimeHandle,
        pool: Arc<UpstreamPool>,
        identity: VerifiedIdentity,
        calls: Arc<AtomicUsize>,
        received: Arc<Mutex<Vec<(ReadResourceRequestParams, RequestMetaObject)>>>,
    }

    fn gateway_config(expose_resources: bool) -> GatewayConfig {
        GatewayConfig {
            upstream: vec![UpstreamConfig {
                enabled: true,
                name: "alpha".into(),
                url: None,
                transport: None,
                socket_path: None,
                headers: Default::default(),
                bearer_token_env: None,
                command: Some("node".into()),
                args: Vec::new(),
                env: Default::default(),
                proxy_resources: true,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                proxy_skills: false,
                expose_skills: None,
                code_mode_hint: None,
                oauth: None,
                imported_from: None,
                priority: 1.0,
            }],
            loadouts: vec![GatewayLoadoutConfig {
                name: "production".into(),
                upstreams: vec!["alpha".into()],
                expose_resources,
                expose_skills: false,
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
        let received = Arc::new(Mutex::new(Vec::new()));
        let pool = Arc::new(UpstreamPool::new());
        pool.install_prompt_server_for_tests(
            "alpha",
            EchoResourceServer {
                calls: Arc::clone(&calls),
                received: Arc::clone(&received),
            },
        )
        .await;
        pool.insert_resource_routes_for_tests(
            "alpha",
            vec![Resource::new(
                "lab://upstream/inner/file:///nested/value",
                "exact",
            )],
        )
        .await;
        let gateway_runtime = GatewayRuntimeHandle::default();
        gateway_runtime.swap(Some(Arc::clone(&pool))).await;
        let gateway_path = directory.path().join("resource-execution.toml");
        let manager = Arc::new(GatewayManager::with_store(
            gateway_path.clone(),
            gateway_runtime.clone(),
            Arc::new(FsGatewayConfigStore::new(gateway_path)),
        ));
        manager.try_seed_config(gateway_config(true)).await.unwrap();
        Fixture {
            _directory: directory,
            runtime,
            manager,
            gateway_runtime,
            pool,
            identity,
            calls,
            received,
        }
    }

    fn input(identity: VerifiedIdentity) -> ResourceReadResolutionInput {
        let mut meta = RequestMetaObject::new();
        meta.insert("trace".into(), serde_json::json!("opaque"));
        ResourceReadResolutionInput::new(
            identity,
            "project-route",
            "https://mcp.example.com/project",
            "bootstrap-default",
            ReadResourceRequestParams::new(
                "lab://upstream/alpha/lab://upstream/inner/file:///nested/value",
            )
            .with_meta(meta),
        )
    }

    #[tokio::test]
    async fn exact_asset_use_resource_read_rewrites_native_and_normalizes_every_content() {
        let fixture = fixture().await;
        let result = read_exact_project_resource(
            &fixture.runtime,
            &fixture.manager,
            input(fixture.identity.clone()),
        )
        .await
        .expect("owner AssetUse read");

        assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
        let received = fixture.received.lock().await;
        assert_eq!(
            received[0].0.uri,
            "lab://upstream/inner/file:///nested/value"
        );
        assert_eq!(
            received[0].1.get("trace"),
            Some(&serde_json::json!("opaque"))
        );
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(
            value["contents"][0]["uri"],
            "lab://upstream/alpha/lab://upstream/inner/file:///nested/value"
        );
        assert_eq!(value["contents"][1]["uri"], value["contents"][0]["uri"]);
    }

    #[tokio::test]
    async fn exact_resource_read_rejects_non_asset_use_and_hidden_route_without_rpc() {
        let viewer = fixture().await;
        viewer
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
            read_exact_project_resource(
                &viewer.runtime,
                &viewer.manager,
                input(viewer.identity.clone()),
            )
            .await,
            Err(ResourceReadResolutionError::Unavailable)
        );
        assert_eq!(viewer.calls.load(Ordering::SeqCst), 0);

        let hidden = fixture().await;
        hidden
            .manager
            .try_seed_config(gateway_config(false))
            .await
            .unwrap();
        assert_eq!(
            read_exact_project_resource(&hidden.runtime, &hidden.manager, input(hidden.identity),)
                .await,
            Err(ResourceReadResolutionError::Unavailable)
        );
        assert_eq!(hidden.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn exact_resource_read_rejects_unknown_and_ui_targets_without_rpc() {
        let fixture = fixture().await;
        for wire_uri in [
            "lab://upstream/alpha/file:///missing",
            "lab://upstream/alpha/UI://widget",
        ] {
            let mut request = input(fixture.identity.clone());
            request.request.uri = wire_uri.to_string();
            assert_eq!(
                read_exact_project_resource(&fixture.runtime, &fixture.manager, request).await,
                Err(ResourceReadResolutionError::Unavailable)
            );
        }
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn exact_resource_read_discards_pool_publication_aba_success_and_failure() {
        for fail in [false, true] {
            let fixture = fixture().await;
            let started = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            fixture
                .pool
                .install_prompt_server_for_tests(
                    "alpha",
                    DelayedResourceServer {
                        started: Arc::clone(&started),
                        release: Arc::clone(&release),
                        fail,
                    },
                )
                .await;
            fixture
                .pool
                .insert_resource_routes_for_tests(
                    "alpha",
                    vec![Resource::new(
                        "lab://upstream/inner/file:///nested/value",
                        "exact",
                    )],
                )
                .await;
            fixture
                .pool
                .set_resource_last_error_for_tests("alpha", Some("sentinel".into()))
                .await;
            let runtime = Arc::clone(&fixture.runtime);
            let manager = Arc::clone(&fixture.manager);
            let request = input(fixture.identity.clone());
            let task = tokio::spawn(async move {
                read_exact_project_resource(&runtime, &manager, request).await
            });
            started.notified().await;
            fixture.gateway_runtime.swap(None).await;
            fixture
                .gateway_runtime
                .swap(Some(Arc::clone(&fixture.pool)))
                .await;
            release.notify_one();
            assert_eq!(
                task.await.unwrap(),
                Err(ResourceReadResolutionError::Unavailable)
            );
            assert_eq!(
                fixture
                    .pool
                    .resource_last_error_for_tests("alpha")
                    .await
                    .as_deref(),
                Some("sentinel")
            );
        }
    }

    #[tokio::test]
    async fn exact_resource_read_rejects_access_route_and_resource_generation_aba() {
        for mutation in ["access", "route", "resource"] {
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
            let initial_runtime_generation = fixture
                .manager
                .published_runtime_loadout_snapshot("production")
                .await
                .generation();
            let started = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            fixture
                .pool
                .install_prompt_server_for_tests(
                    "alpha",
                    DelayedResourceServer {
                        started: Arc::clone(&started),
                        release: Arc::clone(&release),
                        fail: false,
                    },
                )
                .await;
            fixture
                .pool
                .insert_resource_routes_for_tests(
                    "alpha",
                    vec![Resource::new(
                        "lab://upstream/inner/file:///nested/value",
                        "exact",
                    )],
                )
                .await;
            let initial_resource_generation = fixture
                .pool
                .published_resource_catalog()
                .await
                .unwrap()
                .generation();
            let runtime = Arc::clone(&fixture.runtime);
            let manager = Arc::clone(&fixture.manager);
            let request = input(fixture.identity.clone());
            let task = tokio::spawn(async move {
                read_exact_project_resource(&runtime, &manager, request).await
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
                    assert_ne!(
                        fixture
                            .manager
                            .published_runtime_loadout_snapshot("production")
                            .await
                            .generation(),
                        initial_runtime_generation
                    );
                }
                "resource" => {
                    fixture
                        .pool
                        .insert_resource_routes_for_tests(
                            "alpha",
                            vec![Resource::new("file:///other", "other")],
                        )
                        .await;
                    assert_ne!(
                        fixture
                            .pool
                            .published_resource_catalog()
                            .await
                            .unwrap()
                            .generation(),
                        initial_resource_generation
                    );
                    fixture
                        .pool
                        .insert_resource_routes_for_tests(
                            "alpha",
                            vec![Resource::new(
                                "lab://upstream/inner/file:///nested/value",
                                "exact",
                            )],
                        )
                        .await;
                    assert_ne!(
                        fixture
                            .pool
                            .published_resource_catalog()
                            .await
                            .unwrap()
                            .generation(),
                        initial_resource_generation
                    );
                }
                _ => unreachable!(),
            }
            release.notify_one();
            assert_eq!(
                task.await.unwrap(),
                Err(ResourceReadResolutionError::Unavailable),
                "{mutation} ABA must be detected"
            );
        }
    }

    #[tokio::test]
    async fn exact_resource_read_cancellation_never_applies_prepared_outcome() {
        let fixture = fixture().await;
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        fixture
            .pool
            .install_prompt_server_for_tests(
                "alpha",
                DelayedResourceServer {
                    started: Arc::clone(&started),
                    release: Arc::clone(&release),
                    fail: false,
                },
            )
            .await;
        fixture
            .pool
            .insert_resource_routes_for_tests(
                "alpha",
                vec![Resource::new(
                    "lab://upstream/inner/file:///nested/value",
                    "exact",
                )],
            )
            .await;
        fixture
            .pool
            .set_resource_last_error_for_tests("alpha", Some("sentinel".into()))
            .await;
        let runtime = Arc::clone(&fixture.runtime);
        let manager = Arc::clone(&fixture.manager);
        let request = input(fixture.identity);
        let task =
            tokio::spawn(
                async move { read_exact_project_resource(&runtime, &manager, request).await },
            );
        started.notified().await;
        task.abort();
        release.notify_one();
        assert!(task.await.unwrap_err().is_cancelled());
        fixture.gateway_runtime.swap(None).await;
        fixture
            .gateway_runtime
            .swap(Some(Arc::clone(&fixture.pool)))
            .await;
        assert_eq!(
            fixture
                .pool
                .resource_last_error_for_tests("alpha")
                .await
                .as_deref(),
            Some("sentinel")
        );
    }

    #[tokio::test]
    async fn exact_resource_read_maps_stable_upstream_error_without_private_detail() {
        let fixture = fixture().await;
        fixture
            .pool
            .install_prompt_server_for_tests("alpha", FailingResourceServer)
            .await;
        fixture
            .pool
            .insert_resource_routes_for_tests(
                "alpha",
                vec![Resource::new(
                    "lab://upstream/inner/file:///nested/value",
                    "exact",
                )],
            )
            .await;
        let error = read_exact_project_resource(
            &fixture.runtime,
            &fixture.manager,
            input(fixture.identity),
        )
        .await
        .expect_err("stable application error");
        assert_eq!(error, ResourceReadResolutionError::Upstream);
        assert!(
            !error
                .to_string()
                .contains("private stable resource failure")
        );
    }

    #[tokio::test]
    async fn exact_resource_read_maps_oversized_result() {
        let fixture = fixture().await;
        fixture
            .pool
            .install_prompt_server_for_tests("alpha", OversizedResourceServer)
            .await;
        fixture
            .pool
            .insert_resource_routes_for_tests(
                "alpha",
                vec![Resource::new(
                    "lab://upstream/inner/file:///nested/value",
                    "exact",
                )],
            )
            .await;

        assert_eq!(
            read_exact_project_resource(
                &fixture.runtime,
                &fixture.manager,
                input(fixture.identity),
            )
            .await,
            Err(ResourceReadResolutionError::TooLarge)
        );
    }
}
