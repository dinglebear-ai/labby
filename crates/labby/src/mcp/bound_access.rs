//! MCP access-context lifecycle kernel and protected-HTTP shadow binding.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use labby_auth::VerifiedIdentity;
use labby_gateway::gateway::PublishedProjectRouteSnapshot;
use labby_gateway::gateway::manager::GatewayManager;
use thiserror::Error;

use crate::access::{
    AccessRuntime, Permission, ProjectRuntimeMcpCatalogContext, project_runtime_mcp_catalog_context,
};

const BIND_ATTEMPTS: usize = 3;
static NEXT_CONTEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct BoundAccessContextId(u64);

/// Immutable server-owned evidence for one MCP request/session lifecycle.
///
/// This type is deliberately non-`Clone`, non-`Debug`, and non-serializable.
/// Its inputs are server-derived authentication and protected-route facts; MCP
/// request params and `_meta` never participate. It is not a dispatch grant.
/// Resume/session validation remains deferred; the protected HTTP transport
/// wraps this core with the current access-token instance and expiry.
#[allow(dead_code)] // Owned in shadow mode; enforcement consumers land in the next slice.
pub(crate) struct BoundAccessContext {
    id: BoundAccessContextId,
    catalog: ProjectRuntimeMcpCatalogContext,
    route: PublishedProjectRouteSnapshot,
    credential_binding_fingerprint: String,
    safe_fingerprint: String,
}

/// Request-owned protected HTTP binding around the coherent core evidence.
#[allow(dead_code)] // Request-owned in shadow mode; not yet enforced by handlers.
pub(crate) struct TransportBoundAccessContext {
    core: BoundAccessContext,
    credential_instance_fingerprint: String,
    expires_at_unix: u64,
}

pub(crate) struct TransportCredentialBinding {
    fingerprint: String,
    expires_at_unix: u64,
}

#[allow(dead_code)]
impl TransportBoundAccessContext {
    pub(crate) fn new(
        core: BoundAccessContext,
        credential: TransportCredentialBinding,
        now: SystemTime,
    ) -> Result<Self, BoundAccessContextError> {
        if unix_seconds(now)? >= credential.expires_at_unix {
            return Err(BoundAccessContextError::Unavailable);
        }
        Ok(Self {
            core,
            credential_instance_fingerprint: credential.fingerprint,
            expires_at_unix: credential.expires_at_unix,
        })
    }

    pub(crate) fn core(&self) -> &BoundAccessContext {
        &self.core
    }

    pub(crate) fn credential_instance_fingerprint(&self) -> &str {
        &self.credential_instance_fingerprint
    }

    pub(crate) fn validate_not_expired(
        &self,
        now: SystemTime,
    ) -> Result<(), BoundAccessContextError> {
        if unix_seconds(now)? >= self.expires_at_unix {
            Err(BoundAccessContextError::Unavailable)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone)]
#[allow(dead_code)] // Bound payload is intentionally only observed by tests in shadow mode.
pub(crate) enum ProjectAccessObservation {
    Bound(Arc<TransportBoundAccessContext>),
    Unavailable,
}

fn unix_seconds(now: SystemTime) -> Result<u64, BoundAccessContextError> {
    now.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| BoundAccessContextError::Unavailable)
}

pub(crate) fn validate_transport_credential_binding(
    issuer: &str,
    token_id: &str,
    expires_at_unix: usize,
    now: SystemTime,
) -> Result<TransportCredentialBinding, BoundAccessContextError> {
    let expires_at_unix =
        u64::try_from(expires_at_unix).map_err(|_| BoundAccessContextError::Unavailable)?;
    if !labby_auth::jwt::is_canonical_access_token_id(token_id)
        || unix_seconds(now)? >= expires_at_unix
    {
        return Err(BoundAccessContextError::Unavailable);
    }
    Ok(TransportCredentialBinding {
        fingerprint: labby_auth::util::fingerprint(&format!(
            "labby.mcp.transport-binding.v1\0{}:{}{}:{}",
            issuer.len(),
            issuer,
            token_id.len(),
            token_id
        )),
        expires_at_unix,
    })
}

#[allow(dead_code)]
impl BoundAccessContext {
    pub(crate) fn id(&self) -> BoundAccessContextId {
        self.id
    }

    pub(crate) fn catalog(&self) -> &ProjectRuntimeMcpCatalogContext {
        &self.catalog
    }

    pub(crate) fn route(&self) -> &PublishedProjectRouteSnapshot {
        &self.route
    }

    pub(crate) fn safe_fingerprint(&self) -> &str {
        &self.safe_fingerprint
    }

    pub(crate) fn credential_binding_fingerprint(&self) -> &str {
        &self.credential_binding_fingerprint
    }
}

pub(crate) fn attach_project_access_observation(
    extensions: &mut axum::http::Extensions,
    binding: Result<TransportBoundAccessContext, BoundAccessContextError>,
) {
    let observation = match binding {
        Ok(binding) => ProjectAccessObservation::Bound(Arc::new(binding)),
        Err(_) => ProjectAccessObservation::Unavailable,
    };
    extensions.insert(observation);
}

pub(crate) fn project_access_observation_from_mcp_extensions(
    extensions: &rmcp::model::Extensions,
) -> Option<&ProjectAccessObservation> {
    extensions
        .get::<axum::http::request::Parts>()?
        .extensions
        .get::<ProjectAccessObservation>()
}

/// Non-enforcing Project discovery policy observed by `tools/list`.
///
/// `Legacy` means the request was not opted into Project binding. `Unavailable`
/// is an explicit opted-in failure (including expiry) and must never be treated
/// as legacy fallback. Only `Bound` can classify catalog-backed candidates.
pub(crate) enum ProjectToolDiscoveryShadow<'a> {
    Legacy,
    Unavailable,
    Bound(&'a TransportBoundAccessContext),
}

impl ProjectToolDiscoveryShadow<'_> {
    pub(crate) fn state_label_at(&self, now: SystemTime) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Unavailable => "unavailable",
            Self::Bound(binding) if binding.validate_not_expired(now).is_ok() => "bound",
            Self::Bound(_) => "unavailable",
        }
    }

    /// `None` means shadow policy is unavailable/legacy, not allow or deny.
    pub(crate) fn allows_builtin_service(&self, service: &str, now: SystemTime) -> Option<bool> {
        let Self::Bound(binding) = self else {
            return None;
        };
        if binding.validate_not_expired(now).is_err() {
            return None;
        }
        let core = binding.core();
        let route = core.route();
        Some(
            route.effective_loadout().expose_tools
                && route
                    .effective_service_names()
                    .iter()
                    .any(|name| name.as_ref() == service)
                && core
                    .catalog()
                    .catalog()
                    .services()
                    .services()
                    .iter()
                    .any(|published| published.name() == service),
        )
    }

    /// `None` means shadow policy is unavailable/legacy, not allow or deny.
    pub(crate) fn allows_upstream_tool(
        &self,
        upstream: &str,
        tool: &str,
        now: SystemTime,
    ) -> Option<bool> {
        let Self::Bound(binding) = self else {
            return None;
        };
        if binding.validate_not_expired(now).is_err() {
            return None;
        }
        let core = binding.core();
        let route = core.route();
        Some(
            route.effective_loadout().expose_tools
                && route
                    .effective_loadout()
                    .upstreams
                    .iter()
                    .any(|name| name == upstream)
                && core
                    .catalog()
                    .catalog()
                    .tools()
                    .routes()
                    .iter()
                    .any(|route| {
                        route.upstream_name.as_ref() == upstream && route.tool_name.as_ref() == tool
                    }),
        )
    }
}

pub(crate) fn project_tool_discovery_shadow(
    extensions: &rmcp::model::Extensions,
    now: SystemTime,
) -> ProjectToolDiscoveryShadow<'_> {
    match project_access_observation_from_mcp_extensions(extensions) {
        None => ProjectToolDiscoveryShadow::Legacy,
        Some(ProjectAccessObservation::Unavailable) => ProjectToolDiscoveryShadow::Unavailable,
        Some(ProjectAccessObservation::Bound(binding)) => {
            if binding.validate_not_expired(now).is_ok() {
                ProjectToolDiscoveryShadow::Bound(binding)
            } else {
                ProjectToolDiscoveryShadow::Unavailable
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub(crate) enum BoundAccessContextError {
    #[error("MCP access context is unavailable")]
    Unavailable,
    #[error("MCP access context changed during observation")]
    Unstable,
}

pub(crate) async fn bind_access_context(
    runtime: &AccessRuntime,
    manager: &GatewayManager,
    identity: VerifiedIdentity,
    route_name: &str,
    resource: &str,
    project_id: &str,
) -> Result<BoundAccessContext, BoundAccessContextError> {
    let context_identity = identity.clone();
    bind_stable_context(
        || async {
            project_runtime_mcp_catalog_context(
                runtime,
                manager,
                context_identity.clone(),
                project_id,
                Permission::AssetDiscover,
            )
            .await
            .map_err(map_context_error)
        },
        |loadout_name| async move {
            manager
                .published_project_route_snapshot(route_name, project_id, &loadout_name)
                .await
                .map_err(map_route_error)
        },
        identity,
        route_name,
        resource,
    )
    .await
}

/// Three outer attempts cap construction at six stable Project-context reads
/// and six protected-route publications. Each child publication is itself
/// independently bounded; no client parameter or request metadata participates.
async fn bind_stable_context<CF, CFut, RF, RFut>(
    mut read_context: CF,
    mut read_route: RF,
    identity: VerifiedIdentity,
    expected_route_name: &str,
    expected_resource: &str,
) -> Result<BoundAccessContext, BoundAccessContextError>
where
    CF: FnMut() -> CFut,
    CFut: Future<Output = Result<ProjectRuntimeMcpCatalogContext, BoundAccessContextError>>,
    RF: FnMut(String) -> RFut,
    RFut: Future<Output = Result<PublishedProjectRouteSnapshot, BoundAccessContextError>>,
{
    let (second_context, second_route) = observe_coherent_pair(
        || read_context(),
        |loadout| read_route(loadout),
        |context| context.access().loadout_name.clone(),
        ProjectRuntimeMcpCatalogContext::same_publication_as,
        PublishedProjectRouteSnapshot::same_publication_as,
        |context, route| {
            context.catalog().tools().runtime_config_generation()
                == route.runtime_config_generation()
        },
    )
    .await?;
    let access = second_context.access();
    if access.project_id != second_route.project_id()
        || access.loadout_name != second_route.assigned_loadout_name()
        || second_route.route_name() != expected_route_name
        || second_route.resource() != expected_resource
    {
        return Err(BoundAccessContextError::Unavailable);
    }
    let id = NEXT_CONTEXT_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .map(BoundAccessContextId)
        .map_err(|_| BoundAccessContextError::Unavailable)?;
    let credential_binding_fingerprint = identity.safe_binding_fingerprint();
    let safe_fingerprint = labby_auth::util::fingerprint(&format!(
        "{}\0{}\0{}\0{}\0{}",
        id.0,
        access.project_id,
        second_route.route_name(),
        second_route.resource(),
        credential_binding_fingerprint
    ));
    Ok(BoundAccessContext {
        id,
        catalog: second_context,
        route: second_route,
        credential_binding_fingerprint,
        safe_fingerprint,
    })
}

async fn observe_coherent_pair<C, R, CF, CFut, RF, RFut, KF, SC, SR, SG>(
    mut read_context: CF,
    mut read_route: RF,
    route_key: KF,
    same_context: SC,
    same_route: SR,
    same_generation: SG,
) -> Result<(C, R), BoundAccessContextError>
where
    CF: FnMut() -> CFut,
    CFut: Future<Output = Result<C, BoundAccessContextError>>,
    RF: FnMut(String) -> RFut,
    RFut: Future<Output = Result<R, BoundAccessContextError>>,
    KF: Fn(&C) -> String,
    SC: Fn(&C, &C) -> bool,
    SR: Fn(&R, &R) -> bool,
    SG: Fn(&C, &R) -> bool,
{
    for _ in 0..BIND_ATTEMPTS {
        let first_context = read_context().await?;
        let first_route = read_route(route_key(&first_context)).await?;
        let second_context = read_context().await?;
        let second_route = read_route(route_key(&second_context)).await?;
        if same_context(&first_context, &second_context)
            && same_route(&first_route, &second_route)
            && same_generation(&second_context, &second_route)
        {
            return Ok((second_context, second_route));
        }
    }
    Err(BoundAccessContextError::Unstable)
}

fn map_route_error(
    error: labby_gateway::gateway::ProjectRoutePublicationError,
) -> BoundAccessContextError {
    match error {
        labby_gateway::gateway::ProjectRoutePublicationError::Unavailable => {
            BoundAccessContextError::Unavailable
        }
        labby_gateway::gateway::ProjectRoutePublicationError::Unstable => {
            BoundAccessContextError::Unstable
        }
    }
}

fn map_context_error(
    error: crate::access::ProjectRuntimeMcpCatalogError,
) -> BoundAccessContextError {
    match error {
        crate::access::ProjectRuntimeMcpCatalogError::SnapshotUnstable => {
            BoundAccessContextError::Unstable
        }
        _ => BoundAccessContextError::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "proxy-testkit")]
    use std::io;
    #[cfg(feature = "proxy-testkit")]
    use std::sync::Mutex;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    #[cfg(feature = "proxy-testkit")]
    #[derive(Clone, Default)]
    struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

    #[cfg(feature = "proxy-testkit")]
    struct CapturedLogWriter(Arc<Mutex<Vec<u8>>>);

    #[cfg(feature = "proxy-testkit")]
    impl io::Write for CapturedLogWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[cfg(feature = "proxy-testkit")]
    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLogs {
        type Writer = CapturedLogWriter;

        fn make_writer(&'a self) -> Self::Writer {
            CapturedLogWriter(Arc::clone(&self.0))
        }
    }

    #[cfg(feature = "proxy-testkit")]
    async fn list_tools_with_project_observation(
        observation: Option<ProjectAccessObservation>,
    ) -> (rmcp::model::ListToolsResult, String) {
        use std::sync::atomic::AtomicU8;
        use tracing::instrument::WithSubscriber as _;

        let server = crate::mcp::server::LabMcpServer {
            registry: Arc::new(crate::registry::build_default_registry()),
            access_runtime: Arc::new(AccessRuntime::blocked_unavailable()),
            gateway_manager: None,
            peers: Default::default(),
            code_mode_app_state: Default::default(),
            last_listed_tool_contract: Default::default(),
            route_runtime: Default::default(),
            client_registry: Default::default(),
            transport_label: "test",
            logging_level: Arc::new(AtomicU8::new(0)),
            route_scope: crate::mcp::route_scope::McpRouteScope::protected_subset(
                "project-route",
                std::iter::empty::<&str>(),
                ["fs", "setup"],
                false,
            ),
            relay_session_id: 0,
            code_mode_widget_callbacks_enabled_for_test: false,
        };
        let (transport, _client_transport) = tokio::io::duplex(64 * 1024);
        let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, io::Error, _>(
            server, transport, None,
        );
        let mut context = rmcp::service::RequestContext::new(
            rmcp::model::NumberOrString::Number(1),
            running.peer().clone(),
        );
        if let Some(observation) = observation {
            let mut parts = axum::http::Request::new(()).into_parts().0;
            parts.extensions.insert(observation);
            context.extensions.insert(parts);
        }
        let logs = CapturedLogs::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(logs.clone())
            .finish();
        let result = running
            .service()
            .list_tools_impl(None, context)
            .with_subscriber(tracing::Dispatch::new(subscriber))
            .await
            .expect("tools/list");
        let logs = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
        (result, logs)
    }

    #[test]
    fn transport_binding_fingerprint_is_token_specific_redacted_and_expiring() {
        let now = UNIX_EPOCH + std::time::Duration::from_secs(100);
        let first =
            validate_transport_credential_binding("issuer-secret", "jti-secret-a", 101, now)
                .expect("live token");
        let second =
            validate_transport_credential_binding("issuer-secret", "jti-secret-b", 101, now)
                .expect("distinct live token");
        assert_ne!(first.fingerprint, second.fingerprint);
        assert!(!first.fingerprint.contains("issuer-secret"));
        assert!(!first.fingerprint.contains("jti-secret-a"));
        assert_eq!(
            validate_transport_credential_binding("issuer", "jti", 100, now).err(),
            Some(BoundAccessContextError::Unavailable)
        );
        for invalid in ["", " padded", &"x".repeat(257)] {
            assert!(validate_transport_credential_binding("issuer", invalid, 101, now).is_err());
        }
    }

    #[derive(Clone, Copy)]
    struct Observation {
        publication: usize,
        runtime: usize,
    }

    #[tokio::test]
    async fn coherent_pair_retries_cross_generation_then_binds_with_exact_counts() {
        let contexts = Arc::new(AtomicUsize::new(0));
        let routes = Arc::new(AtomicUsize::new(0));
        let context_reads = Arc::clone(&contexts);
        let route_reads = Arc::clone(&routes);
        let result = observe_coherent_pair(
            move || {
                let call = context_reads.fetch_add(1, Ordering::SeqCst);
                async move {
                    Ok(Observation {
                        publication: call / 2,
                        runtime: usize::from(call >= 2),
                    })
                }
            },
            move |_| {
                let call = route_reads.fetch_add(1, Ordering::SeqCst);
                async move {
                    Ok(Observation {
                        publication: call / 2,
                        runtime: 1,
                    })
                }
            },
            |_| "production".to_string(),
            |first, second| first.publication == second.publication,
            |first, second| first.publication == second.publication,
            |context, route| context.runtime == route.runtime,
        )
        .await
        .expect("second attempt converges");
        assert_eq!(result.0.runtime, 1);
        assert_eq!(contexts.load(Ordering::SeqCst), 4);
        assert_eq!(routes.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn coherent_pair_sustained_cross_generation_stops_at_bound() {
        let contexts = Arc::new(AtomicUsize::new(0));
        let routes = Arc::new(AtomicUsize::new(0));
        let context_reads = Arc::clone(&contexts);
        let route_reads = Arc::clone(&routes);
        let result = observe_coherent_pair(
            move || {
                let call = context_reads.fetch_add(1, Ordering::SeqCst);
                async move {
                    Ok(Observation {
                        publication: call / 2,
                        runtime: 0,
                    })
                }
            },
            move |_| {
                let call = route_reads.fetch_add(1, Ordering::SeqCst);
                async move {
                    Ok(Observation {
                        publication: call / 2,
                        runtime: 1,
                    })
                }
            },
            |_| "production".to_string(),
            |first, second| first.publication == second.publication,
            |first, second| first.publication == second.publication,
            |context, route| context.runtime == route.runtime,
        )
        .await;
        assert_eq!(result.err(), Some(BoundAccessContextError::Unstable));
        assert_eq!(contexts.load(Ordering::SeqCst), 6);
        assert_eq!(routes.load(Ordering::SeqCst), 6);
    }

    #[tokio::test]
    async fn coherent_pair_context_failure_precedes_route_read() {
        let routes = Arc::new(AtomicUsize::new(0));
        let route_reads = Arc::clone(&routes);
        let result = observe_coherent_pair::<Observation, Observation, _, _, _, _, _, _, _, _>(
            || async { Err(BoundAccessContextError::Unavailable) },
            move |_| {
                route_reads.fetch_add(1, Ordering::SeqCst);
                async {
                    Ok(Observation {
                        publication: 0,
                        runtime: 0,
                    })
                }
            },
            |_| "production".to_string(),
            |first, second| first.publication == second.publication,
            |first, second| first.publication == second.publication,
            |context, route| context.runtime == route.runtime,
        )
        .await;
        assert_eq!(result.err(), Some(BoundAccessContextError::Unavailable));
        assert_eq!(routes.load(Ordering::SeqCst), 0);
    }

    #[cfg(feature = "proxy-testkit")]
    #[tokio::test]
    async fn real_binding_owns_exact_facts_and_remains_immutable() {
        use labby_auth::Authenticator;
        use labby_gateway::gateway::config_store::FsGatewayConfigStore;
        use labby_gateway::gateway::manager::GatewayRuntimeHandle;
        use labby_gateway::upstream::pool::UpstreamPool;
        use labby_runtime::gateway_config::{
            GatewayConfig, GatewayLoadoutConfig, ProtectedGatewaySubsetTarget,
            ProtectedMcpRouteConfig, ProtectedMcpRouteTarget, VirtualServerConfig,
            VirtualServerSurfacesConfig,
        };

        use crate::access::{AssignProjectLoadoutInput, BootstrapOwnerInput};

        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let runtime = AccessRuntime::initialize(directory.path().join("access.db")).await;
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

        let gateway_runtime = GatewayRuntimeHandle::default();
        gateway_runtime
            .swap(Some(Arc::new(UpstreamPool::new())))
            .await;
        let gateway_path = directory.path().join("bound-context.toml");
        let manager = GatewayManager::with_store(
            gateway_path.clone(),
            gateway_runtime,
            Arc::new(FsGatewayConfigStore::new(gateway_path)),
        )
        .with_builtin_service_registry(Arc::new(crate::registry::build_default_registry()));
        let config = || GatewayConfig {
            loadouts: vec![GatewayLoadoutConfig {
                name: "production".into(),
                services: vec!["fs-primary".into()],
                ..Default::default()
            }],
            virtual_servers: vec![VirtualServerConfig {
                id: "fs-primary".into(),
                service: "fs".into(),
                enabled: true,
                surfaces: VirtualServerSurfacesConfig {
                    mcp: true,
                    ..Default::default()
                },
                mcp_policy: None,
            }],
            protected_mcp_routes: vec![ProtectedMcpRouteConfig {
                name: "project-route".into(),
                enabled: true,
                public_host: "MCP.Example.com.".into(),
                public_path: "/project".into(),
                upstream: None,
                backend_url: String::new(),
                backend_mcp_path: "/mcp".into(),
                scopes: vec![],
                health_path: None,
                target: Some(ProtectedMcpRouteTarget::GatewaySubset(
                    ProtectedGatewaySubsetTarget {
                        project_id: Some("bootstrap-default".into()),
                        loadout: Some("production".into()),
                        ..Default::default()
                    },
                )),
            }],
            ..Default::default()
        };
        manager.try_seed_config(config()).await.unwrap();

        let first = bind_access_context(
            &runtime,
            &manager,
            identity.clone(),
            "project-route",
            "https://mcp.example.com/project",
            "bootstrap-default",
        )
        .await
        .expect("first binding");
        let second = bind_access_context(
            &runtime,
            &manager,
            identity.clone(),
            "project-route",
            "https://mcp.example.com/project",
            "bootstrap-default",
        )
        .await
        .expect("second binding");

        assert_eq!(
            first.catalog().access().permission,
            Permission::AssetDiscover
        );
        assert_eq!(first.catalog().access().project_id, "bootstrap-default");
        assert_eq!(first.route().route_name(), "project-route");
        assert_eq!(first.route().resource(), "https://mcp.example.com/project");
        assert!(first.id() != second.id());
        assert_ne!(first.safe_fingerprint(), second.safe_fingerprint());
        assert_eq!(
            first.credential_binding_fingerprint(),
            identity.safe_binding_fingerprint()
        );

        let now = UNIX_EPOCH + std::time::Duration::from_secs(100);
        let credential = validate_transport_credential_binding("issuer", "request-jti", 101, now)
            .expect("transport credential");
        let transport = TransportBoundAccessContext::new(second, credential, now)
            .expect("still live at attachment");
        let request = {
            let mut request = axum::http::Request::new(());
            attach_project_access_observation(request.extensions_mut(), Ok(transport));
            request
        };
        let (parts, _) = request.into_parts();
        let mut extensions = rmcp::model::Extensions::new();
        extensions.insert(parts);
        let ProjectAccessObservation::Bound(observed) =
            project_access_observation_from_mcp_extensions(&extensions)
                .expect("bound observation crosses HTTP Parts")
        else {
            panic!("expected bound observation");
        };
        assert_eq!(
            observed.credential_instance_fingerprint(),
            labby_auth::util::fingerprint(concat!(
                "labby.mcp.transport-binding.v1\0",
                "6:issuer11:request-jti"
            ))
        );
        let shadow = project_tool_discovery_shadow(&extensions, now);
        assert_eq!(shadow.state_label_at(now), "bound");
        assert_eq!(shadow.allows_builtin_service("fs", now), Some(true));
        assert_eq!(shadow.allows_builtin_service("setup", now), Some(false));
        assert_eq!(
            shadow.allows_upstream_tool("unpublished", "missing", now),
            Some(false)
        );
        assert_eq!(
            shadow.state_label_at(UNIX_EPOCH + std::time::Duration::from_secs(101)),
            "unavailable"
        );
        let live_core = bind_access_context(
            &runtime,
            &manager,
            identity.clone(),
            "project-route",
            "https://mcp.example.com/project",
            "bootstrap-default",
        )
        .await
        .expect("live shadow core");
        let live_now = SystemTime::now();
        let live_expiry = usize::try_from(unix_seconds(live_now).unwrap() + 3_600).unwrap();
        let live_credential =
            validate_transport_credential_binding("issuer", "live-jti", live_expiry, live_now)
                .expect("live credential");
        let bound_observation = ProjectAccessObservation::Bound(Arc::new(
            TransportBoundAccessContext::new(live_core, live_credential, live_now)
                .expect("live transport"),
        ));
        let (legacy_result, _) = list_tools_with_project_observation(None).await;
        let (unavailable_result, _) =
            list_tools_with_project_observation(Some(ProjectAccessObservation::Unavailable)).await;
        let (bound_result, bound_logs) =
            list_tools_with_project_observation(Some(bound_observation)).await;
        assert_eq!(
            serde_json::to_value(&legacy_result).unwrap(),
            serde_json::to_value(&unavailable_result).unwrap(),
            "explicit shadow unavailability must not filter tools/list"
        );
        assert_eq!(
            serde_json::to_value(&legacy_result).unwrap(),
            serde_json::to_value(&bound_result).unwrap(),
            "Bound shadow differences must not filter tools/list"
        );
        assert!(
            legacy_result
                .tools
                .iter()
                .any(|tool| tool.name.as_ref() == "setup"),
            "the unchanged response must retain a service absent from the Bound catalog"
        );
        assert!(bound_logs.contains("project_shadow_state=\"bound\""));
        assert!(bound_logs.contains("project_shadow_would_suppress_tool_count=1"));
        for secret in [
            "bootstrap-default",
            "project-route",
            "live-jti",
            "server-credential",
            "fs-primary",
        ] {
            assert!(!bound_logs.contains(secret), "shadow log leaked {secret}");
        }

        let mut unavailable_request = axum::http::Request::new(());
        attach_project_access_observation(
            unavailable_request.extensions_mut(),
            Err(BoundAccessContextError::Unavailable),
        );
        assert!(matches!(
            unavailable_request
                .extensions()
                .get::<ProjectAccessObservation>(),
            Some(ProjectAccessObservation::Unavailable)
        ));
        assert!(
            axum::http::Request::new(())
                .extensions()
                .get::<ProjectAccessObservation>()
                .is_none()
        );
        assert_eq!(
            project_tool_discovery_shadow(&rmcp::model::Extensions::new(), now).state_label_at(now),
            "legacy"
        );

        let expiring_core = bind_access_context(
            &runtime,
            &manager,
            identity.clone(),
            "project-route",
            "https://mcp.example.com/project",
            "bootstrap-default",
        )
        .await
        .expect("expiring core");
        let expiring = validate_transport_credential_binding("issuer", "expiring-jti", 101, now)
            .expect("valid at preflight");
        assert!(matches!(
            TransportBoundAccessContext::new(
                expiring_core,
                expiring,
                UNIX_EPOCH + std::time::Duration::from_secs(101),
            ),
            Err(BoundAccessContextError::Unavailable)
        ));

        manager.try_seed_config(config()).await.unwrap();
        assert_eq!(first.route().resource(), "https://mcp.example.com/project");
        let mismatch = bind_access_context(
            &runtime,
            &manager,
            identity,
            "project-route",
            "https://wrong.example/project",
            "bootstrap-default",
        )
        .await
        .err()
        .expect("stable mismatch");
        assert_eq!(mismatch, BoundAccessContextError::Unavailable);
        assert_eq!(mismatch.to_string(), "MCP access context is unavailable");
    }
}
