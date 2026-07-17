//! Resource handler bodies (`list_resources`, `read_resource`).
//!
//! Extracted from `server.rs` (bead `lab-kvji.24.1.3`) as inherent
//! `impl LabMcpServer` methods. The `ServerHandler` trait impl in
//! `server.rs` keeps one-line delegators.
//!
//! `read_resource_impl` keeps the prefix-dispatch skeleton + the local
//! `lab://catalog` / `lab://<svc>/actions` branch; the three proxy
//! branches live in `resource_proxy.rs` and are reached via the same
//! guard ordering as the original (gateway → upstream → subject-scoped).
//!
//! No behavior change — relocation only.

use std::time::Instant;

use rmcp::ErrorData;
use rmcp::RoleServer;
use rmcp::model::{
    ListResourcesResult, Meta, PaginatedRequestParams, ReadResourceRequestParams,
    ReadResourceResult, Resource, ResourceContents,
};
use rmcp::service::RequestContext;
use serde_json::{Value, json};

#[cfg(feature = "gateway")]
pub(crate) use crate::app_assets::{
    ADD_SERVER_APP_SKYBRIDGE_URI, ADD_SERVER_APP_URI, GATEWAY_STATUS_APP_SKYBRIDGE_URI,
    GATEWAY_STATUS_APP_URI,
};
pub(crate) use crate::app_assets::{
    SERVER_LOGS_APP_SKYBRIDGE_URI, SERVER_LOGS_APP_URI, SERVER_LOGS_APP_URI_PREFIX,
};
#[cfg(feature = "gateway")]
use crate::mcp::catalog::{ADD_SERVER_TOOL_NAME, GATEWAY_STATUS_TOOL_NAME};
use crate::mcp::catalog::{CODE_MODE_TOOL_NAME, SERVER_LOGS_TOOL_NAME};
#[cfg(feature = "gateway")]
use crate::mcp::context::oauth_upstream_subject_for_request;
use crate::mcp::context::{auth_context_from_extensions, code_mode_read_scope_allowed};
use crate::mcp::logging::{DispatchLogOutcome, LoggingLevel};
use crate::mcp::pagination::{PageCollector, error_kind as pagination_error_kind};
use crate::mcp::server::LabMcpServer;

/// MCP Apps (Claude / SEP-1724) MIME — bound via the tool's `_meta.ui.resourceUri`.
pub(crate) const CODE_MODE_APP_MIME: &str = "text/html;profile=mcp-app";
/// OpenAI Apps (ChatGPT / Codex) MIME — bound via the tool's `openai/outputTemplate`.
/// Same HTML body; a distinct URI + MIME so the Claude resource stays untouched.
pub(crate) const CODE_MODE_APP_SKYBRIDGE_MIME: &str = "text/html+skybridge";
/// URI namespace reserved for Lab's own Code Mode app resources, served locally.
/// Any other `ui://` is an upstream mcp-ui widget resource routed to its peer.
pub(crate) const CODE_MODE_APP_URI_PREFIX: &str = "ui://lab/code-mode/";
pub(crate) const CODE_MODE_APP_URI: &str = "ui://lab/code-mode/codemode";
pub(crate) const CODE_MODE_HISTORY_APP_URI: &str = "ui://lab/code-mode/history";
/// OpenAI Apps skybridge variants — same HTML, served under the skybridge MIME.
pub(crate) const CODE_MODE_APP_SKYBRIDGE_URI: &str = "ui://lab/code-mode/codemode.skybridge";
/// Host runtime a Code Mode widget resource targets. The runtime is the single
/// discriminant: it derives the served MIME, whether the resource is listed, and
/// which tool `_meta` key the resource URI is exposed under — so those
/// projections can't drift apart.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodeModeRuntime {
    /// Anthropic MCP Apps (Claude): `text/html;profile=mcp-app`, listed in
    /// `resources/list`, bound via the tool's `_meta.ui.resourceUri`.
    McpApp,
    /// OpenAI Apps (ChatGPT / Codex): `text/html+skybridge`, unlisted — reached
    /// directly via the tool's `openai/outputTemplate`.
    Skybridge,
}

impl CodeModeRuntime {
    const fn mime(self) -> &'static str {
        match self {
            Self::McpApp => CODE_MODE_APP_MIME,
            Self::Skybridge => CODE_MODE_APP_SKYBRIDGE_MIME,
        }
    }

    /// Only MCP Apps resources appear in `resources/list`; skybridge variants are
    /// discovered via the tool's `openai/outputTemplate`, keeping the Claude
    /// surface unchanged.
    const fn listed(self) -> bool {
        matches!(self, Self::McpApp)
    }
}

pub(crate) struct AppResourceDescriptor {
    pub(crate) uri: &'static str,
    pub(crate) name: &'static str,
    pub(crate) runtime: CodeModeRuntime,
    /// Tool this widget binds to, or `None` for the history widget (not tool-
    /// bound). `runtime` selects which `_meta` key the URI is exposed under.
    pub(crate) tool_name: Option<&'static str>,
    pub(crate) resource_description: &'static str,
    pub(crate) skybridge_widget_description: Option<&'static str>,
}

pub(crate) const CODE_MODE_APP_RESOURCE_DESCRIPTORS: &[AppResourceDescriptor] = &[
    AppResourceDescriptor {
        uri: CODE_MODE_APP_URI,
        name: "code-mode/codemode",
        runtime: CodeModeRuntime::McpApp,
        tool_name: Some(CODE_MODE_TOOL_NAME),
        resource_description: "Read-only MCP App for Code Mode call traces",
        skybridge_widget_description: None,
    },
    AppResourceDescriptor {
        uri: CODE_MODE_HISTORY_APP_URI,
        name: "code-mode/history",
        runtime: CodeModeRuntime::McpApp,
        tool_name: None,
        resource_description: "Read-only MCP App for Code Mode call traces",
        skybridge_widget_description: None,
    },
    AppResourceDescriptor {
        uri: CODE_MODE_APP_SKYBRIDGE_URI,
        name: "code-mode/codemode.skybridge",
        runtime: CodeModeRuntime::Skybridge,
        tool_name: Some(CODE_MODE_TOOL_NAME),
        resource_description: "Read-only MCP App for Code Mode call traces",
        skybridge_widget_description: Some(
            "Live Code Mode call trace — upstream tool calls, catalog search matches, and recent gateway history.",
        ),
    },
];

const CODE_MODE_APP_FALLBACK_HTML: &str = include_str!("assets/code_mode_app.html");
const SERVER_LOGS_APP_FALLBACK_HTML: &str = crate::app_assets::SERVER_LOGS_APP_HTML;
#[cfg(feature = "gateway")]
const ADD_SERVER_APP_FALLBACK_HTML: &str = crate::app_assets::ADD_SERVER_APP_HTML;
#[cfg(feature = "gateway")]
const GATEWAY_STATUS_APP_FALLBACK_HTML: &str = crate::app_assets::GATEWAY_STATUS_APP_HTML;

pub(crate) const SERVER_LOGS_APP_RESOURCE_DESCRIPTORS: &[AppResourceDescriptor] = &[
    AppResourceDescriptor {
        uri: SERVER_LOGS_APP_URI,
        name: "server-logs/viewer",
        runtime: CodeModeRuntime::McpApp,
        tool_name: Some(SERVER_LOGS_TOOL_NAME),
        resource_description: "Admin MCP App for Labby server process logs",
        skybridge_widget_description: None,
    },
    AppResourceDescriptor {
        uri: SERVER_LOGS_APP_SKYBRIDGE_URI,
        name: "server-logs/viewer.skybridge",
        runtime: CodeModeRuntime::Skybridge,
        tool_name: Some(SERVER_LOGS_TOOL_NAME),
        resource_description: "Admin MCP App for Labby server process logs",
        skybridge_widget_description: Some(
            "Admin viewer for Labby's rolling server process logs with level, service, action, kind, and text filters.",
        ),
    },
];

#[cfg(feature = "gateway")]
pub(crate) const ADD_SERVER_APP_RESOURCE_DESCRIPTORS: &[AppResourceDescriptor] = &[
    AppResourceDescriptor {
        uri: ADD_SERVER_APP_URI,
        name: "gateway/add-server",
        runtime: CodeModeRuntime::McpApp,
        tool_name: Some(ADD_SERVER_TOOL_NAME),
        resource_description: "Admin MCP App for adding an upstream server to Labby",
        skybridge_widget_description: None,
    },
    AppResourceDescriptor {
        uri: ADD_SERVER_APP_SKYBRIDGE_URI,
        name: "gateway/add-server.skybridge",
        runtime: CodeModeRuntime::Skybridge,
        tool_name: Some(ADD_SERVER_TOOL_NAME),
        resource_description: "Admin MCP App for adding an upstream server to Labby",
        skybridge_widget_description: Some(
            "Connect and test a remote or local MCP server, then add it to the Labby gateway catalog.",
        ),
    },
];

#[cfg(feature = "gateway")]
pub(crate) const GATEWAY_STATUS_APP_RESOURCE_DESCRIPTORS: &[AppResourceDescriptor] = &[
    AppResourceDescriptor {
        uri: GATEWAY_STATUS_APP_URI,
        name: "gateway/status",
        runtime: CodeModeRuntime::McpApp,
        tool_name: Some(GATEWAY_STATUS_TOOL_NAME),
        resource_description: "Admin MCP App for live gateway upstream status",
        skybridge_widget_description: None,
    },
    AppResourceDescriptor {
        uri: GATEWAY_STATUS_APP_SKYBRIDGE_URI,
        name: "gateway/status.skybridge",
        runtime: CodeModeRuntime::Skybridge,
        tool_name: Some(GATEWAY_STATUS_TOOL_NAME),
        resource_description: "Admin MCP App for live gateway upstream status",
        skybridge_widget_description: Some(
            "Live connection status, capabilities, and warnings for Labby gateway upstream MCP servers.",
        ),
    },
];

/// FNV-1a over the bundled widget HTML, evaluated at compile time. Changes iff
/// the HTML bytes change, so it is a stable per-build cache-bust key.
const fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    hash
}

/// Cache-bust token for the Code Mode widget URIs.
///
/// MCP Apps / OpenAI Apps hosts cache the widget resource by its `resourceUri`,
/// and the base `ui://lab/code-mode/*` URIs never change between builds — so a
/// host that cached pre-fix HTML keeps serving it even after labby is rebuilt
/// and restarted. Appending a content hash of the bundled HTML as `?v=<hash>`
/// makes the advertised URI change exactly when the widget changes, forcing the
/// host to refetch. The read path strips this suffix before matching descriptors,
/// so the base URIs stay directly readable.
/// Hash the fallback HTML plus the host bridge injected into the served resource.
fn bridged_app_content_version(html: &str) -> String {
    let input = format!("{html}\n{}", crate::app_assets::LABBY_APP_HOST_JS);
    format!("{:016x}", fnv1a_64(input.as_bytes()))
}

static CODE_MODE_APP_VERSION: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!("{:016x}", fnv1a_64(CODE_MODE_APP_FALLBACK_HTML.as_bytes()))
});
static SERVER_LOGS_APP_VERSION: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| bridged_app_content_version(SERVER_LOGS_APP_FALLBACK_HTML));
#[cfg(feature = "gateway")]
static ADD_SERVER_APP_VERSION: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| bridged_app_content_version(ADD_SERVER_APP_FALLBACK_HTML));
#[cfg(feature = "gateway")]
static GATEWAY_STATUS_APP_VERSION: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| bridged_app_content_version(GATEWAY_STATUS_APP_FALLBACK_HTML));

#[derive(Clone, Copy)]
struct OwnedAppRegistration {
    descriptors: &'static [AppResourceDescriptor],
    html: &'static str,
    version: &'static std::sync::LazyLock<String>,
}

impl OwnedAppRegistration {
    /// Resolve either a canonical or cache-busted URI to its descriptor.
    fn descriptor(self, uri: &str) -> Option<&'static AppResourceDescriptor> {
        app_descriptor_for_uri(self.descriptors, uri)
    }

    /// Add the registration's content-derived cache-bust token to a base URI.
    fn versioned_uri(self, base: &str) -> String {
        format!("{base}?v={}", self.version.as_str())
    }

    /// Build the MCP resource metadata for one registered runtime variant.
    fn resource(self, descriptor: &AppResourceDescriptor) -> Resource {
        let uri = self.versioned_uri(descriptor.uri);
        Resource::new(uri.clone(), descriptor.name.to_string())
            .with_description(descriptor.resource_description)
            .with_mime_type(descriptor.runtime.mime())
            .with_meta(app_resource_meta_for_descriptor(&uri, descriptor))
    }

    /// Build the resource-list entries hosts are expected to discover.
    fn listed_resources(self) -> Vec<Resource> {
        self.descriptors
            .iter()
            .filter(|descriptor| descriptor.runtime.listed())
            .map(|descriptor| self.resource(descriptor))
            .collect()
    }

    /// Find a tool-bound URI for a particular host runtime.
    fn uri_for_tool(self, runtime: CodeModeRuntime, tool_name: &str) -> Option<String> {
        self.descriptors
            .iter()
            .find(|descriptor| {
                descriptor.runtime == runtime && descriptor.tool_name == Some(tool_name)
            })
            .map(|descriptor| self.versioned_uri(descriptor.uri))
    }

    /// Inline the shared host bridge into this registration's fallback HTML.
    fn inline_html(self, descriptor: &AppResourceDescriptor) -> Result<String, String> {
        inline_app_host_script(self.html, descriptor)
    }
}

/// Return the shared Code Mode app registration.
fn code_mode_app() -> OwnedAppRegistration {
    OwnedAppRegistration {
        descriptors: CODE_MODE_APP_RESOURCE_DESCRIPTORS,
        html: CODE_MODE_APP_FALLBACK_HTML,
        version: &CODE_MODE_APP_VERSION,
    }
}

/// Return the Server Logs app registration.
fn server_logs_app() -> OwnedAppRegistration {
    OwnedAppRegistration {
        descriptors: SERVER_LOGS_APP_RESOURCE_DESCRIPTORS,
        html: SERVER_LOGS_APP_FALLBACK_HTML,
        version: &SERVER_LOGS_APP_VERSION,
    }
}

#[cfg(feature = "gateway")]
/// Return the gateway Add Server app registration.
fn add_server_app() -> OwnedAppRegistration {
    OwnedAppRegistration {
        descriptors: ADD_SERVER_APP_RESOURCE_DESCRIPTORS,
        html: ADD_SERVER_APP_FALLBACK_HTML,
        version: &ADD_SERVER_APP_VERSION,
    }
}

#[cfg(feature = "gateway")]
/// Return the gateway upstream status app registration.
fn gateway_status_app() -> OwnedAppRegistration {
    OwnedAppRegistration {
        descriptors: GATEWAY_STATUS_APP_RESOURCE_DESCRIPTORS,
        html: GATEWAY_STATUS_APP_FALLBACK_HTML,
        version: &GATEWAY_STATUS_APP_VERSION,
    }
}

/// Strip the `?v=<hash>` cache-bust suffix so a versioned URI matches its base
/// descriptor. A base URI (no query) is returned unchanged.
fn strip_app_version(uri: &str) -> &str {
    uri.split_once('?').map_or(uri, |(base, _)| base)
}

impl LabMcpServer {
    pub(crate) async fn list_resources_impl(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let start = Instant::now();
        let subject = self.request_subject_log_tag(&context);
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "list_resources",
            subject,
            "dispatch start"
        );
        let auth = auth_context_from_extensions(&context.extensions);
        let mut resources = match PageCollector::new(request) {
            Ok(collector) => collector,
            Err(error) => {
                let elapsed_ms = start.elapsed().as_millis();
                let kind = pagination_error_kind(&error);
                tracing::warn!(
                    surface = "mcp",
                    service = "labby",
                    action = "list_resources",
                    subject,
                    elapsed_ms,
                    kind,
                    "resource list failed"
                );
                self.emit_dispatch_notification(
                    &context,
                    "lab",
                    "list_resources",
                    elapsed_ms,
                    DispatchLogOutcome::Failure {
                        level: LoggingLevel::Warning,
                        kind,
                    },
                )
                .await;
                return Err(error);
            }
        };

        resources.accept(
            Resource::new("lab://catalog", "catalog")
                .with_description("Full discovery document for all services")
                .with_mime_type("application/json"),
        );

        if !resources.finished()
            && code_mode_app_resources_visible(
                self.code_mode_visibility().await.exposes_synthetic_tools(),
                auth,
            )
        {
            for resource in code_mode_app_resources() {
                resources.accept(resource);
                if resources.finished() {
                    break;
                }
            }
        }

        #[cfg(feature = "gateway")]
        if !resources.finished()
            && admin_app_resources_visible(auth)
            && self.gateway_status_app_available_on_mcp().await
        {
            for resource in gateway_status_app_resources() {
                resources.accept(resource);
                if resources.finished() {
                    break;
                }
            }
        }

        if !resources.finished()
            && admin_app_resources_visible(auth)
            && self.route_scope.allows_service(SERVER_LOGS_TOOL_NAME)
            && self.service_visible_on_mcp(SERVER_LOGS_TOOL_NAME).await
        {
            for resource in server_logs_app_resources() {
                resources.accept(resource);
                if resources.finished() {
                    break;
                }
            }
        }

        #[cfg(feature = "gateway")]
        if !resources.finished()
            && admin_app_resources_visible(auth)
            && self.add_server_app_available_on_mcp().await
        {
            for resource in add_server_app_resources() {
                resources.accept(resource);
                if resources.finished() {
                    break;
                }
            }
        }

        if !resources.finished() {
            for svc in self.registry.services() {
                if self.route_scope.allows_service(svc.name)
                    && self.service_visible_on_mcp(svc.name).await
                {
                    let uri = format!("lab://{}/actions", svc.name);
                    let name = format!("{}/actions", svc.name);
                    resources.accept(
                        Resource::new(uri, name)
                            .with_description(format!("Action list for {}", svc.name))
                            .with_mime_type("application/json"),
                    );
                    if resources.finished() {
                        break;
                    }
                }
            }
        }

        #[cfg(feature = "gateway")]
        if !resources.finished()
            && let Some(pool) = self.current_upstream_pool().await
        {
            for resource in pool
                .gateway_synthetic_resources_allowed(self.route_scope.allowed_upstreams())
                .await
            {
                resources.accept(resource);
                if resources.finished() {
                    break;
                }
            }
            if !resources.finished() {
                for resource in pool
                    .list_upstream_resources_allowed(self.route_scope.allowed_upstreams())
                    .await
                {
                    resources.accept(resource);
                    if resources.finished() {
                        break;
                    }
                }
            }
            if !resources.finished()
                && let Some(oauth_subject) =
                    oauth_upstream_subject_for_request(auth, self.request_subject(&context))
            {
                let configs = self.route_scoped_oauth_upstream_configs().await;
                let mut scoped_resources = pool
                    .subject_scoped_resources(&configs, oauth_subject.as_ref())
                    .await;
                scoped_resources.retain(|resource| {
                    resource
                        .uri
                        .strip_prefix("lab://upstream/")
                        .and_then(|rest| rest.split('/').next())
                        .is_none_or(|upstream| self.route_scope.allows_upstream(upstream))
                });
                for resource in scoped_resources {
                    resources.accept(resource);
                    if resources.finished() {
                        break;
                    }
                }
            }
        }

        let (resources, next_cursor) = match resources.finish() {
            Ok(page) => page,
            Err(error) => {
                let elapsed_ms = start.elapsed().as_millis();
                let kind = pagination_error_kind(&error);
                tracing::warn!(
                    surface = "mcp",
                    service = "labby",
                    action = "list_resources",
                    subject,
                    elapsed_ms,
                    kind,
                    "resource list failed"
                );
                self.emit_dispatch_notification(
                    &context,
                    "lab",
                    "list_resources",
                    elapsed_ms,
                    DispatchLogOutcome::Failure {
                        level: LoggingLevel::Warning,
                        kind,
                    },
                )
                .await;
                return Err(error);
            }
        };

        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "list_resources",
            subject,
            elapsed_ms,
            "resource list ok"
        );
        self.emit_dispatch_notification(
            &context,
            "lab",
            "list_resources",
            elapsed_ms,
            DispatchLogOutcome::Success,
        )
        .await;

        let mut result = ListResourcesResult::with_all_items(resources);
        result.next_cursor = next_cursor;
        Ok(result)
    }

    pub(crate) async fn read_resource_impl(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        let start = Instant::now();
        let subject = self.request_subject_log_tag(&context);
        let uri = &request.uri;
        #[cfg(feature = "gateway")]
        let resource_uri_log =
            crate::dispatch::upstream::pool::redact_resource_uri_for_logging(uri);
        #[cfg(not(feature = "gateway"))]
        let resource_uri_log = uri.to_string();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "read_resource",
            subject,
            resource_uri = %resource_uri_log,
            "dispatch start"
        );

        // Branch 0: MCP Apps UI resources. This must precede all lab://
        // fallbacks so ui:// has its own exact lookup semantics.
        //
        // Local Code Mode app resources own the `ui://lab/code-mode/*` namespace
        // and are served from the bundled HTML.
        if uri.starts_with(CODE_MODE_APP_URI_PREFIX) {
            return self
                .read_code_mode_app_resource_impl(uri, &subject, start, &context)
                .await;
        }
        if uri.starts_with(SERVER_LOGS_APP_URI_PREFIX) {
            return self
                .read_server_logs_app_resource_impl(uri, &subject, start, &context)
                .await;
        }
        #[cfg(feature = "gateway")]
        if uri.starts_with(ADD_SERVER_APP_URI) {
            return self
                .read_add_server_app_resource_impl(
                    uri,
                    &resource_uri_log,
                    &subject,
                    start,
                    &context,
                )
                .await;
        }
        #[cfg(feature = "gateway")]
        if uri.starts_with(GATEWAY_STATUS_APP_URI) {
            return self
                .read_gateway_status_app_resource_impl(
                    uri,
                    &resource_uri_log,
                    &subject,
                    start,
                    &context,
                )
                .await;
        }
        // Any other `ui://` is an upstream MCP Apps (mcp-ui) widget resource
        // (referenced by a tool result's `_meta.ui.resourceUri`): reverse-look-up
        // the owning upstream peer via the pool and forward the read under the
        // native `ui://` URI. These widgets are surfaced through the Code Mode
        // synthetic surface, so gate them behind the same read scope as Lab's own
        // Code Mode app resources rather than leaving them ungated.
        #[cfg(feature = "gateway")]
        if uri.starts_with("ui://") {
            let auth = auth_context_from_extensions(&context.extensions);
            if !code_mode_read_scope_allowed(auth) {
                return Err(ErrorData::invalid_params(
                    "UI resources require one of scopes: lab:read, lab, lab:admin",
                    Some(json!({
                        "kind": "forbidden",
                        "required_scopes": ["lab:read", "lab", "lab:admin"],
                    })),
                ));
            }
            if let Some(pool) = self.current_upstream_pool().await {
                return self
                    .read_upstream_ui_resource_impl(&pool, uri, &subject, start, &context)
                    .await;
            }
            return Err(ErrorData::resource_not_found(
                format!("unknown UI resource: {uri}"),
                None,
            ));
        }

        // Branch 1: local per-service action resources. This must precede the
        // `lab://gateway/*` proxy branch so `lab://gateway/actions` remains the
        // built-in gateway service catalog resource, not a gateway synthetic
        // resource lookup.
        if let Some(service) = uri
            .strip_prefix("lab://")
            .and_then(|value| value.strip_suffix("/actions"))
        {
            if !self.route_scope.allows_service(service) {
                let elapsed_ms = start.elapsed().as_millis();
                let message = format!("service `{service}` is not exposed on this MCP route");
                tracing::warn!(
                    surface = "mcp",
                    service,
                    action = "read_resource",
                    subject,
                    route_scope = %self.route_scope.label(),
                    resource_uri = %resource_uri_log,
                    elapsed_ms,
                    kind = "route_scope_denied",
                    error = %message,
                    "MCP resource read denied by protected route scope"
                );
                self.emit_dispatch_notification(
                    &context,
                    "lab",
                    "read_resource",
                    elapsed_ms,
                    DispatchLogOutcome::Failure {
                        level: LoggingLevel::Warning,
                        kind: "route_scope_denied",
                    },
                )
                .await;
                return Err(ErrorData::invalid_params(
                    message,
                    Some(json!({
                        "kind": "route_scope_denied",
                        "service": service,
                    })),
                ));
            }

            let json = self.service_actions_json(service).await;
            return self
                .read_local_json_resource(json, uri, &subject, start, &context)
                .await;
        }

        // Branch 2: gateway-synthetic resources.
        #[cfg(feature = "gateway")]
        if uri.starts_with("lab://gateway/") {
            return self
                .read_gateway_resource_impl(uri, &subject, start, &context)
                .await;
        }

        // Branch 3: raw upstream resource proxy.
        #[cfg(feature = "gateway")]
        if let Some(pool) = self.current_upstream_pool().await
            && uri.starts_with("lab://upstream/")
        {
            return self
                .read_upstream_resource_impl(&pool, uri, &subject, start, &context)
                .await;
        }

        // Branch 4: subject-scoped upstream resource proxy.
        #[cfg(feature = "gateway")]
        let auth = auth_context_from_extensions(&context.extensions);
        #[cfg(feature = "gateway")]
        if let Some(oauth_subject) =
            oauth_upstream_subject_for_request(auth, self.request_subject(&context))
            && let Some(pool) = self.current_upstream_pool().await
            && let Some(upstream_name) = uri
                .strip_prefix("lab://upstream/")
                .and_then(|rest| rest.split('/').next())
            && self.route_scope.allows_upstream(upstream_name)
            && let Some(config) = self.oauth_upstream_config(upstream_name).await
        {
            return self
                .read_subject_scoped_resource_impl(
                    &pool,
                    &config,
                    oauth_subject.as_ref(),
                    uri,
                    &subject,
                    start,
                    &context,
                )
                .await;
        }

        // Local branch: lab://catalog + lab://<svc>/actions.
        let json = if uri == "lab://catalog" {
            self.catalog_json().await
        } else {
            return Err(ErrorData::resource_not_found(
                format!("unknown resource: {uri}"),
                None,
            ));
        };

        self.read_local_json_resource(json, uri, &subject, start, &context)
            .await
    }

    async fn read_local_json_resource(
        &self,
        json: anyhow::Result<Value>,
        uri: &str,
        subject: &str,
        start: Instant,
        context: &RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        match json {
            Ok(value) => {
                let text = match serde_json::to_string_pretty(&value) {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::error!(
                            surface = "mcp",
                            service = "labby",
                            action = "read_resource",
                            subject,
                            error = %e,
                            "failed to serialize resource"
                        );
                        return Err(ErrorData::internal_error(
                            format!("failed to serialize resource: {e}"),
                            None,
                        ));
                    }
                };
                let elapsed_ms = start.elapsed().as_millis();
                tracing::info!(
                    surface = "mcp",
                    service = "labby",
                    action = "read_resource",
                    subject,
                    elapsed_ms,
                    "resource read ok"
                );
                self.emit_dispatch_notification(
                    &context,
                    "lab",
                    "read_resource",
                    elapsed_ms,
                    DispatchLogOutcome::Success,
                )
                .await;
                Ok(ReadResourceResult::new(vec![
                    ResourceContents::text(text, uri.to_string())
                        .with_mime_type("application/json"),
                ]))
            }
            Err(e) => {
                let elapsed_ms = start.elapsed().as_millis();
                tracing::error!(
                    surface = "mcp",
                    service = "labby",
                    action = "read_resource",
                    elapsed_ms,
                    kind = "internal_error",
                    "resource read failed"
                );
                self.emit_dispatch_notification(
                    &context,
                    "lab",
                    "read_resource",
                    elapsed_ms,
                    DispatchLogOutcome::Failure {
                        level: LoggingLevel::Error,
                        kind: "internal_error",
                    },
                )
                .await;
                Err(ErrorData::internal_error(e.to_string(), None))
            }
        }
    }

    async fn read_code_mode_app_resource_impl(
        &self,
        uri: &str,
        subject: &str,
        start: Instant,
        context: &RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        if !self.code_mode_visibility().await.exposes_synthetic_tools() {
            return Err(ErrorData::resource_not_found(
                format!("unknown UI resource: {uri}"),
                None,
            ));
        }
        let auth = auth_context_from_extensions(&context.extensions);
        if !code_mode_read_scope_allowed(auth) {
            let elapsed_ms = start.elapsed().as_millis();
            tracing::warn!(
                surface = "mcp",
                service = "labby",
                action = "read_resource",
                subject,
                elapsed_ms,
                kind = "forbidden",
                resource_uri = uri,
                "code mode app resource denied by scope"
            );
            self.emit_dispatch_notification(
                context,
                "lab",
                "read_resource",
                elapsed_ms,
                DispatchLogOutcome::Failure {
                    level: LoggingLevel::Warning,
                    kind: "forbidden",
                },
            )
            .await;
            return Err(ErrorData::invalid_params(
                "Code Mode app resources require one of scopes: lab:read, lab, lab:admin",
                Some(json!({
                    "kind": "forbidden",
                    "required_scopes": ["lab:read", "lab", "lab:admin"],
                })),
            ));
        }
        let history = if strip_app_version(uri) == CODE_MODE_HISTORY_APP_URI {
            #[cfg(feature = "gateway")]
            match &self.gateway_manager {
                Some(manager) if self.route_scope.protected_history_label().is_some() => {
                    let label = self.route_scope.protected_history_label();
                    Some(json!({
                        "kind": "code_mode_history",
                        "entries": manager.code_mode_history_snapshot_for_route_scope(label.as_deref()).await,
                    }))
                }
                Some(manager) => Some(json!({
                    "kind": "code_mode_history",
                    "entries": manager.code_mode_history_snapshot().await,
                })),
                None => Some(json!({ "kind": "code_mode_history", "entries": [] })),
            }
            #[cfg(not(feature = "gateway"))]
            {
                Some(json!({ "kind": "code_mode_history", "entries": [] }))
            }
        } else {
            None
        };
        let descriptor = app_descriptor_for_uri(CODE_MODE_APP_RESOURCE_DESCRIPTORS, uri)
            .ok_or_else(|| {
                ErrorData::resource_not_found(format!("unknown UI resource: {uri}"), None)
            })?;
        let html = code_mode_app_html_for_descriptor(history.as_ref());
        let runtime = descriptor.runtime;
        let mime_type = runtime.mime();
        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "read_resource",
            subject,
            elapsed_ms,
            resource_uri = uri,
            mime_type,
            html_bytes = html.len(),
            versioned = uri.contains("?v="),
            "code mode app resource read ok"
        );
        self.emit_dispatch_notification(
            context,
            "lab",
            "read_resource",
            elapsed_ms,
            DispatchLogOutcome::Success,
        )
        .await;

        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(html, uri.to_string())
                .with_mime_type(mime_type)
                .with_meta(app_resource_meta_for_descriptor(uri, descriptor)),
        ]))
    }

    async fn read_server_logs_app_resource_impl(
        &self,
        uri: &str,
        subject: &str,
        start: Instant,
        context: &RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        if !self.route_scope.allows_service(SERVER_LOGS_TOOL_NAME)
            || !self.service_visible_on_mcp(SERVER_LOGS_TOOL_NAME).await
        {
            return Err(ErrorData::resource_not_found(
                format!("unknown UI resource: {uri}"),
                None,
            ));
        }
        let auth = auth_context_from_extensions(&context.extensions);
        if !admin_app_resources_visible(auth) {
            let elapsed_ms = start.elapsed().as_millis();
            tracing::warn!(
                surface = "mcp",
                service = "labby",
                action = "read_resource",
                subject,
                elapsed_ms,
                kind = "forbidden",
                resource_uri = uri,
                "server logs app resource denied by scope"
            );
            self.emit_dispatch_notification(
                context,
                "lab",
                "read_resource",
                elapsed_ms,
                DispatchLogOutcome::Failure {
                    level: LoggingLevel::Warning,
                    kind: "forbidden",
                },
            )
            .await;
            return Err(ErrorData::invalid_params(
                "Server log app resources require scope: lab:admin",
                Some(json!({
                    "kind": "forbidden",
                    "required_scopes": ["lab:admin"],
                })),
            ));
        }

        let app = server_logs_app();
        let descriptor = app.descriptor(uri).ok_or_else(|| {
            ErrorData::resource_not_found(format!("unknown UI resource: {uri}"), None)
        })?;
        let html = app
            .inline_html(descriptor)
            .map_err(|message| ErrorData::internal_error(message, None))?;
        let runtime = descriptor.runtime;
        let mime_type = runtime.mime();
        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "read_resource",
            subject,
            elapsed_ms,
            resource_uri = uri,
            mime_type,
            html_bytes = html.len(),
            versioned = uri.contains("?v="),
            "server logs app resource read ok"
        );
        self.emit_dispatch_notification(
            context,
            "lab",
            "read_resource",
            elapsed_ms,
            DispatchLogOutcome::Success,
        )
        .await;

        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(html, uri.to_string())
                .with_mime_type(mime_type)
                .with_meta(app_resource_meta_for_descriptor(uri, descriptor)),
        ]))
    }

    #[cfg(feature = "gateway")]
    async fn read_add_server_app_resource_impl(
        &self,
        uri: &str,
        resource_uri_log: &str,
        subject: &str,
        start: Instant,
        context: &RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        if !self.add_server_app_available_on_mcp().await {
            let elapsed_ms = start.elapsed().as_millis();
            tracing::warn!(
                surface = "mcp",
                service = "labby",
                action = "read_resource",
                subject,
                elapsed_ms,
                kind = "not_found",
                resource_uri = resource_uri_log,
                "add server app resource unavailable"
            );
            self.emit_dispatch_notification(
                context,
                "lab",
                "read_resource",
                elapsed_ms,
                DispatchLogOutcome::Failure {
                    level: LoggingLevel::Warning,
                    kind: "not_found",
                },
            )
            .await;
            return Err(ErrorData::resource_not_found(
                format!("unknown UI resource: {uri}"),
                None,
            ));
        }
        let auth = auth_context_from_extensions(&context.extensions);
        if !admin_app_resources_visible(auth) {
            let elapsed_ms = start.elapsed().as_millis();
            tracing::warn!(
                surface = "mcp",
                service = "labby",
                action = "read_resource",
                subject,
                elapsed_ms,
                kind = "forbidden",
                resource_uri = resource_uri_log,
                "add server app resource denied by scope"
            );
            self.emit_dispatch_notification(
                context,
                "lab",
                "read_resource",
                elapsed_ms,
                DispatchLogOutcome::Failure {
                    level: LoggingLevel::Warning,
                    kind: "forbidden",
                },
            )
            .await;
            return Err(ErrorData::invalid_params(
                "Add Server app resources require scope: lab:admin",
                Some(json!({
                    "kind": "forbidden",
                    "required_scopes": ["lab:admin"],
                })),
            ));
        }
        let app = add_server_app();
        let descriptor = app.descriptor(uri).ok_or_else(|| {
            ErrorData::resource_not_found(format!("unknown UI resource: {uri}"), None)
        })?;
        let html = app
            .inline_html(descriptor)
            .map_err(|message| ErrorData::internal_error(message, None))?;
        let mime_type = descriptor.runtime.mime();
        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "read_resource",
            subject,
            elapsed_ms,
            resource_uri = resource_uri_log,
            mime_type,
            html_bytes = html.len(),
            "add server app resource read ok"
        );
        self.emit_dispatch_notification(
            context,
            "lab",
            "read_resource",
            elapsed_ms,
            DispatchLogOutcome::Success,
        )
        .await;
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(html, uri.to_string())
                .with_mime_type(mime_type)
                .with_meta(app_resource_meta_for_descriptor(uri, descriptor)),
        ]))
    }

    #[cfg(feature = "gateway")]
    async fn read_gateway_status_app_resource_impl(
        &self,
        uri: &str,
        resource_uri_log: &str,
        subject: &str,
        start: Instant,
        context: &RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        if !self.gateway_status_app_available_on_mcp().await {
            let elapsed_ms = start.elapsed().as_millis();
            tracing::warn!(
                surface = "mcp",
                service = "labby",
                action = "read_resource",
                subject,
                elapsed_ms,
                kind = "not_found",
                resource_uri = resource_uri_log,
                "gateway status app resource unavailable"
            );
            self.emit_dispatch_notification(
                context,
                "lab",
                "read_resource",
                elapsed_ms,
                DispatchLogOutcome::Failure {
                    level: LoggingLevel::Warning,
                    kind: "not_found",
                },
            )
            .await;
            return Err(ErrorData::resource_not_found(
                format!("unknown UI resource: {uri}"),
                None,
            ));
        }
        if !admin_app_resources_visible(auth_context_from_extensions(&context.extensions)) {
            let elapsed_ms = start.elapsed().as_millis();
            tracing::warn!(
                surface = "mcp",
                service = "labby",
                action = "read_resource",
                subject,
                elapsed_ms,
                kind = "forbidden",
                resource_uri = resource_uri_log,
                "gateway status app resource denied by scope"
            );
            self.emit_dispatch_notification(
                context,
                "lab",
                "read_resource",
                elapsed_ms,
                DispatchLogOutcome::Failure {
                    level: LoggingLevel::Warning,
                    kind: "forbidden",
                },
            )
            .await;
            return Err(ErrorData::invalid_params(
                "Gateway Status app resources require scope: lab:admin",
                Some(json!({
                    "kind": "forbidden",
                    "required_scopes": ["lab:admin"],
                })),
            ));
        }
        let app = gateway_status_app();
        let descriptor = app.descriptor(uri).ok_or_else(|| {
            ErrorData::resource_not_found(format!("unknown UI resource: {uri}"), None)
        })?;
        let html = app
            .inline_html(descriptor)
            .map_err(|message| ErrorData::internal_error(message, None))?;
        let mime_type = descriptor.runtime.mime();
        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "read_resource",
            subject,
            elapsed_ms,
            resource_uri = resource_uri_log,
            mime_type,
            html_bytes = html.len(),
            "gateway status app resource read ok"
        );
        self.emit_dispatch_notification(
            context,
            "lab",
            "read_resource",
            elapsed_ms,
            DispatchLogOutcome::Success,
        )
        .await;
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(html, uri.to_string())
                .with_mime_type(mime_type)
                .with_meta(app_resource_meta_for_descriptor(uri, descriptor)),
        ]))
    }
}

#[cfg(test)]
fn code_mode_app_html(uri: &str, history: Option<&Value>) -> Result<String, String> {
    if app_descriptor_for_uri(CODE_MODE_APP_RESOURCE_DESCRIPTORS, uri).is_none() {
        return Err(format!("unknown UI resource: {uri}"));
    }
    Ok(code_mode_app_html_for_descriptor(history))
}

fn code_mode_app_html_for_descriptor(history: Option<&Value>) -> String {
    let mut html = CODE_MODE_APP_FALLBACK_HTML.to_string();
    if let Some(snapshot) = history {
        let injected = format!(
            "window.__LAB_CODE_MODE_INITIAL_TRACE__ = {};",
            snapshot.to_string().replace('<', "\\u003c")
        );
        html = html.replace("window.__LAB_CODE_MODE_INITIAL_TRACE__ = null;", &injected);
    }
    html
}

#[cfg(test)]
fn server_logs_app_html(uri: &str) -> Result<String, String> {
    let app = server_logs_app();
    let Some(descriptor) = app.descriptor(uri) else {
        return Err(format!("unknown UI resource: {uri}"));
    };
    app.inline_html(descriptor)
}

/// Replace the external host-script marker with the embedded bridge runtime.
fn inline_app_host_script(
    html: &str,
    descriptor: &AppResourceDescriptor,
) -> Result<String, String> {
    const HOST_SCRIPT_MARKER: &str = r#"<script src="/apps/assets/labby-app-host.js"></script>"#;
    if !html.contains(HOST_SCRIPT_MARKER) {
        return Err("missing Labby app host script marker".to_string());
    }
    let mcp_resource_flag = if descriptor.runtime == CodeModeRuntime::McpApp {
        "window.__LABBY_MCP_RESOURCE=true;"
    } else {
        ""
    };
    Ok(html.replace(
        HOST_SCRIPT_MARKER,
        &format!(
            "<script>{mcp_resource_flag}{}</script>",
            crate::app_assets::LABBY_APP_HOST_JS
        ),
    ))
}

/// Resolve a canonical or cache-busted URI within a descriptor table.
fn app_descriptor_for_uri<'a>(
    descriptors: &'a [AppResourceDescriptor],
    uri: &str,
) -> Option<&'a AppResourceDescriptor> {
    let base = strip_app_version(uri);
    descriptors.iter().find(|descriptor| descriptor.uri == base)
}

#[cfg(test)]
fn versioned_app_uri(base: &str) -> String {
    code_mode_app().versioned_uri(base)
}

/// Host runtime a Code Mode app URI targets. Callers must pass a table URI; an
/// un-tabled URI is a programming error because runtime selects MIME,
/// listed-ness, and tool binding.
#[cfg(test)]
fn code_mode_app_runtime_for_uri(uri: &str) -> CodeModeRuntime {
    app_runtime_for_uri(uri, CODE_MODE_APP_RESOURCE_DESCRIPTORS, "Code Mode")
}

#[cfg(test)]
fn app_runtime_for_uri(
    uri: &str,
    descriptors: &[AppResourceDescriptor],
    label: &'static str,
) -> CodeModeRuntime {
    app_descriptor_for_uri(descriptors, uri)
        .unwrap_or_else(|| panic!("{label} app runtime lookup called with un-tabled URI: {uri}"))
        .runtime
}

/// Whether Code Mode app resources are readable by the current caller.
fn code_mode_app_resources_visible(
    exposes_synthetic_tools: bool,
    auth: Option<&labby_auth::auth_context::AuthContext>,
) -> bool {
    exposes_synthetic_tools && code_mode_read_scope_allowed(auth)
}

/// Whether admin-only Lab-owned app resources are readable by this caller.
pub(crate) fn admin_app_resources_visible(
    auth: Option<&labby_auth::auth_context::AuthContext>,
) -> bool {
    auth.is_none_or(|auth| auth.scopes.iter().any(|scope| scope == "lab:admin"))
}

/// Build the discoverable Code Mode app resources.
fn code_mode_app_resources() -> Vec<Resource> {
    code_mode_app().listed_resources()
}

/// Build the discoverable Server Logs app resources.
fn server_logs_app_resources() -> Vec<Resource> {
    server_logs_app().listed_resources()
}

#[cfg(feature = "gateway")]
/// Build the discoverable Add Server app resources.
fn add_server_app_resources() -> Vec<Resource> {
    add_server_app().listed_resources()
}

#[cfg(feature = "gateway")]
/// Build the discoverable Gateway Status app resources.
fn gateway_status_app_resources() -> Vec<Resource> {
    gateway_status_app().listed_resources()
}

/// MCP Apps (Claude) widget URI for a tool — backs `_meta.ui.resourceUri`.
///
/// Carries the `?v=<hash>` cache-bust suffix so a rebuilt widget forces the host
/// to refetch instead of rendering its cached copy of the previous build.
pub(crate) fn code_mode_app_resource_uri_for_tool(tool_name: &str) -> Option<String> {
    code_mode_app().uri_for_tool(CodeModeRuntime::McpApp, tool_name)
}

/// OpenAI Apps (ChatGPT / Codex) widget URI for a tool — backs `openai/outputTemplate`.
///
/// Carries the same `?v=<hash>` cache-bust suffix as the MCP Apps URI.
pub(crate) fn code_mode_app_skybridge_uri_for_tool(tool_name: &str) -> Option<String> {
    code_mode_app().uri_for_tool(CodeModeRuntime::Skybridge, tool_name)
}

/// MCP Apps widget URI for the server log viewer tool.
pub(crate) fn server_logs_app_resource_uri_for_tool(tool_name: &str) -> Option<String> {
    server_logs_app().uri_for_tool(CodeModeRuntime::McpApp, tool_name)
}

/// OpenAI Apps skybridge widget URI for the server log viewer tool.
pub(crate) fn server_logs_app_skybridge_uri_for_tool(tool_name: &str) -> Option<String> {
    server_logs_app().uri_for_tool(CodeModeRuntime::Skybridge, tool_name)
}

#[cfg(feature = "gateway")]
pub(crate) fn add_server_app_resource_uri_for_tool(tool_name: &str) -> Option<String> {
    add_server_app().uri_for_tool(CodeModeRuntime::McpApp, tool_name)
}

#[cfg(feature = "gateway")]
pub(crate) fn add_server_app_skybridge_uri_for_tool(tool_name: &str) -> Option<String> {
    add_server_app().uri_for_tool(CodeModeRuntime::Skybridge, tool_name)
}

#[cfg(feature = "gateway")]
pub(crate) fn gateway_status_app_resource_uri_for_tool(tool_name: &str) -> Option<String> {
    gateway_status_app().uri_for_tool(CodeModeRuntime::McpApp, tool_name)
}

#[cfg(feature = "gateway")]
pub(crate) fn gateway_status_app_skybridge_uri_for_tool(tool_name: &str) -> Option<String> {
    gateway_status_app().uri_for_tool(CodeModeRuntime::Skybridge, tool_name)
}

#[cfg(test)]
pub(crate) fn code_mode_app_resource_meta(uri: &str) -> Meta {
    app_resource_meta(uri, CODE_MODE_APP_RESOURCE_DESCRIPTORS)
}

#[cfg(test)]
fn app_resource_meta(uri: &str, descriptors: &[AppResourceDescriptor]) -> Meta {
    let descriptor = app_descriptor_for_uri(descriptors, uri)
        .unwrap_or_else(|| panic!("app resource meta lookup called with un-tabled URI: {uri}"));
    app_resource_meta_for_descriptor(uri, descriptor)
}

fn app_resource_meta_for_descriptor(uri: &str, descriptor: &AppResourceDescriptor) -> Meta {
    build_app_resource_meta(
        uri,
        descriptor.runtime,
        descriptor.skybridge_widget_description,
    )
}

fn build_app_resource_meta(
    uri: &str,
    runtime: CodeModeRuntime,
    skybridge_widget_description: Option<&'static str>,
) -> Meta {
    let mut meta = serde_json::Map::new();
    meta.insert(
        "ui".to_string(),
        json!({
            "resourceUri": uri,
            "mimeTypes": [runtime.mime()],
            "csp": {
                "connectDomains": [],
                "resourceDomains": [],
                "frameDomains": [],
            },
            "prefersBorder": false,
        }),
    );
    if runtime == CodeModeRuntime::Skybridge
        && let Some(description) = skybridge_widget_description
    {
        meta.insert("openai/widgetDescription".to_string(), json!(description));
    }
    Meta(meta)
}

#[cfg(all(test, feature = "gateway"))]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use crate::dispatch::upstream::pool::{
        InProcessConnector, InProcessRegistration, UpstreamConnection, UpstreamPool,
    };
    use crate::dispatch::upstream::types::UpstreamRuntimeMetadata;
    use futures::future::BoxFuture;
    use rmcp::model::{ListResourcesResult, ServerCapabilities, ServerInfo, Tool};
    use rmcp::service::{Peer, RequestContext};
    use rmcp::{RoleClient, ServerHandler, ServiceExt};
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;

    const UPSTREAM_UI_URI: &str = "ui://quick-shell/app.html";
    const UPSTREAM_UI_TOOL_NAME: &str = "quick_shell_ui";

    struct UpstreamUiResourceServer;

    impl ServerHandler for UpstreamUiResourceServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_resources().build())
        }

        async fn list_resources(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListResourcesResult, ErrorData> {
            Ok(ListResourcesResult::with_all_items(vec![
                Resource::new(UPSTREAM_UI_URI, "quick-shell/app")
                    .with_mime_type("text/html;profile=mcp-app"),
            ]))
        }

        async fn read_resource(
            &self,
            params: ReadResourceRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<ReadResourceResult, ErrorData> {
            if params.uri != UPSTREAM_UI_URI {
                return Err(ErrorData::resource_not_found(
                    format!("unknown upstream UI resource: {}", params.uri),
                    None,
                ));
            }

            Ok(ReadResourceResult::new(vec![
                ResourceContents::text("<main>quick shell widget</main>", params.uri)
                    .with_mime_type("text/html;profile=mcp-app"),
            ]))
        }
    }

    async fn code_mode_server() -> LabMcpServer {
        code_mode_server_with_scope(crate::mcp::route_scope::McpRouteScope::Root).await
    }

    async fn code_mode_server_with_scope(
        route_scope: crate::mcp::route_scope::McpRouteScope,
    ) -> LabMcpServer {
        let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
        let manager = Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                std::path::PathBuf::from("config.toml"),
                runtime,
            ),
        );
        manager
            .seed_config_unchecked_for_tests(
                crate::config::LabConfig {
                    code_mode: crate::config::CodeModeConfig {
                        enabled: true,
                        ..crate::config::CodeModeConfig::default()
                    },
                    ..crate::config::LabConfig::default()
                }
                .to_gateway_config(),
            )
            .await;
        LabMcpServer {
            registry: Arc::new(crate::registry::ToolRegistry::new()),
            gateway_manager: Some(manager),
            peers: Arc::new(tokio::sync::RwLock::new(Vec::new())),
            client_registry: Default::default(),
            transport_label: "test",
            logging_level: Arc::new(std::sync::atomic::AtomicU8::new(
                crate::mcp::logging::logging_level_rank(LoggingLevel::Emergency),
            )),
            route_scope,
            relay_session_id: 0,
            code_mode_widget_callbacks_enabled_for_test: false,
        }
    }

    async fn resource_scope_server(
        route_scope: crate::mcp::route_scope::McpRouteScope,
    ) -> LabMcpServer {
        let mut server = code_mode_server_with_scope(route_scope).await;
        server.registry = Arc::new(crate::registry::build_default_registry());
        server
    }

    async fn code_mode_server_with_upstream_ui_resource() -> LabMcpServer {
        static ACTIONS: &[labby_primitives::action::ActionSpec] =
            &[labby_primitives::action::ActionSpec {
                name: "terminal.open",
                description: "Open terminal",
                destructive: false,
                requires_admin: false,
                params: &[],
                returns: "object",
            }];

        let mut registry = crate::registry::ToolRegistry::new();
        registry.register(crate::registry::RegisteredService {
            name: "quick_shell",
            description: "Quick shell",
            category: "test",
            kind: crate::registry::RegisteredServiceKind::BootstrapOperator,
            status: "available",
            actions: ACTIONS,
            dispatch: noop_dispatch,
        });

        let connector: InProcessConnector = Arc::new(|service| {
            let future: BoxFuture<'static, anyhow::Result<InProcessRegistration>> =
                Box::pin(async move {
                    let upstream_name: Arc<str> = Arc::from(service.service_name());
                    let mut tool = Tool::new(
                        UPSTREAM_UI_TOOL_NAME.to_string(),
                        "Quick shell UI",
                        Arc::new(serde_json::Map::new()),
                    );
                    tool.meta = Some(Meta(serde_json::Map::from_iter([(
                        "ui".to_string(),
                        json!({ "resourceUri": UPSTREAM_UI_URI }),
                    )])));
                    Ok(InProcessRegistration {
                        connection: Some(upstream_ui_connection().await),
                        tools: vec![tool],
                        entry_name: Arc::clone(&upstream_name),
                        upstream_name: upstream_name.to_string(),
                    })
                });
            future
        });

        let pool = Arc::new(UpstreamPool::new().with_in_process_connector(connector));
        pool.register_in_process_service_peers(&registry).await;
        pool.list_upstream_resources().await;

        let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
        runtime.swap(Some(pool)).await;
        let manager = Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                std::path::PathBuf::from("config.toml"),
                runtime,
            ),
        );
        manager
            .seed_config_unchecked_for_tests(
                crate::config::LabConfig {
                    code_mode: crate::config::CodeModeConfig {
                        enabled: true,
                        ..crate::config::CodeModeConfig::default()
                    },
                    upstream: vec![crate::config::UpstreamConfig {
                        enabled: true,
                        name: "quick_shell".to_string(),
                        url: None,
                        bearer_token_env: None,
                        command: Some("in-process".to_string()),
                        args: Vec::new(),
                        env: BTreeMap::new(),
                        proxy_resources: true,
                        proxy_prompts: false,
                        expose_tools: None,
                        expose_resources: None,
                        expose_prompts: None,
                        code_mode_hint: None,
                        oauth: None,
                        imported_from: None,
                        priority: 1.0,
                    }],
                    ..crate::config::LabConfig::default()
                }
                .to_gateway_config(),
            )
            .await;

        LabMcpServer {
            registry: Arc::new(registry),
            gateway_manager: Some(manager),
            peers: Arc::new(tokio::sync::RwLock::new(Vec::new())),
            client_registry: Default::default(),
            transport_label: "test",
            logging_level: Arc::new(std::sync::atomic::AtomicU8::new(
                crate::mcp::logging::logging_level_rank(LoggingLevel::Emergency),
            )),
            route_scope: crate::mcp::route_scope::McpRouteScope::Root,
            relay_session_id: 0,
            code_mode_widget_callbacks_enabled_for_test: false,
        }
    }

    async fn upstream_ui_connection() -> UpstreamConnection {
        let (server_transport, client_transport) = tokio::io::duplex(256 * 1024);
        let server_task = tokio::spawn(async move {
            let running = UpstreamUiResourceServer
                .serve(server_transport)
                .await
                .expect("upstream UI server starts");
            running.waiting().await.expect("upstream UI server runs");
        });
        let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
            .serve(client_transport)
            .await
            .expect("upstream UI client starts");
        let peer = client_service.peer().clone();
        UpstreamConnection::new(
            client_service,
            Some(server_task),
            peer,
            UpstreamRuntimeMetadata::default(),
        )
    }

    fn noop_dispatch(
        _action: String,
        _params: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, crate::dispatch::error::ToolError>> + Send>>
    {
        Box::pin(async { Ok(Value::Null) })
    }

    fn large_resource_server(service_count: usize) -> LabMcpServer {
        let mut registry = crate::registry::ToolRegistry::new();
        static ACTIONS: &[labby_primitives::action::ActionSpec] =
            &[labby_primitives::action::ActionSpec {
                name: "thing.list",
                description: "List things",
                destructive: false,
                requires_admin: false,
                params: &[],
                returns: "object",
            }];
        for index in 0..service_count {
            let name = Box::leak(format!("resource_service_{index:03}").into_boxed_str());
            registry.register(crate::registry::RegisteredService {
                name,
                description: "Synthetic service",
                category: "test",
                kind: crate::registry::RegisteredServiceKind::BootstrapOperator,
                status: "available",
                actions: ACTIONS,
                dispatch: noop_dispatch,
            });
        }
        LabMcpServer {
            registry: Arc::new(registry),
            gateway_manager: None,
            peers: Arc::new(tokio::sync::RwLock::new(Vec::new())),
            client_registry: Default::default(),
            transport_label: "test",
            logging_level: Arc::new(std::sync::atomic::AtomicU8::new(
                crate::mcp::logging::logging_level_rank(LoggingLevel::Emergency),
            )),
            route_scope: crate::mcp::route_scope::McpRouteScope::Root,
            relay_session_id: 0,
            code_mode_widget_callbacks_enabled_for_test: false,
        }
    }

    fn scoped_context(peer: Peer<RoleServer>, scopes: &[&str]) -> RequestContext<RoleServer> {
        let mut context = RequestContext::new(rmcp::model::NumberOrString::Number(1), peer);
        let mut parts = axum::http::Request::new(()).into_parts().0;
        parts
            .extensions
            .insert(labby_auth::auth_context::AuthContext {
                sub: "reader".to_string(),
                actor_key: None,
                scopes: scopes.iter().map(|scope| scope.to_string()).collect(),
                issuer: "https://lab.example.com".to_string(),
                via_session: true,
                csrf_token: None,
                email: None,
            });
        context.extensions.insert(parts);
        context
    }

    #[test]
    fn code_mode_app_resource_meta_uses_mcp_app_mime_and_csp() {
        let meta = code_mode_app_resource_meta(CODE_MODE_APP_URI);
        assert_eq!(
            meta.0["ui"]["resourceUri"].as_str(),
            Some(CODE_MODE_APP_URI)
        );
        assert_eq!(
            meta.0["ui"]["mimeTypes"][0].as_str(),
            Some(CODE_MODE_APP_MIME)
        );
        assert_eq!(meta.0["ui"]["prefersBorder"].as_bool(), Some(false));
        assert!(meta.0.get("csp").is_none(), "CSP belongs under _meta.ui");
        assert!(
            meta.0.get("prefersBorder").is_none(),
            "border preference belongs under _meta.ui"
        );
        assert_eq!(meta.0["ui"]["csp"]["connectDomains"], json!([]));
        assert_eq!(meta.0["ui"]["csp"]["resourceDomains"], json!([]));
        assert_eq!(meta.0["ui"]["csp"]["frameDomains"], json!([]));
    }

    #[tokio::test]
    async fn list_resources_only_lists_code_mode_apps_for_read_scope() {
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            code_mode_server().await,
            transport,
            None,
        );

        let denied = running
            .service()
            .list_resources_impl(None, scoped_context(running.peer().clone(), &["profile"]))
            .await
            .expect("list resources without scope");
        assert!(
            denied
                .resources
                .iter()
                .all(|resource| !resource.uri.starts_with("ui://lab/code-mode/")),
            "listed Code Mode UI resources without read scope"
        );

        let allowed = running
            .service()
            .list_resources_impl(None, scoped_context(running.peer().clone(), &["lab:read"]))
            .await
            .expect("list resources with scope");
        let code_mode_uris = allowed
            .resources
            .iter()
            .filter(|resource| resource.uri.starts_with("ui://lab/code-mode/"))
            .map(|resource| resource.uri.clone())
            .collect::<Vec<_>>();
        // Advertised URIs carry the `?v=<hash>` cache-bust suffix; compare bases.
        assert_eq!(
            code_mode_uris
                .iter()
                .map(|uri| strip_app_version(uri))
                .collect::<Vec<_>>(),
            vec![CODE_MODE_APP_URI, CODE_MODE_HISTORY_APP_URI]
        );
        assert!(
            code_mode_uris.iter().all(|uri| uri.contains("?v=")),
            "advertised Code Mode URIs must carry a cache-bust token: {code_mode_uris:?}"
        );
    }

    #[tokio::test]
    async fn code_mode_app_resource_does_not_shadow_upstream_mcp_ui_resources() {
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            code_mode_server_with_upstream_ui_resource().await,
            transport,
            None,
        );
        let context = scoped_context(running.peer().clone(), &["lab:read"]);

        let tools = running
            .service()
            .list_tools_impl(None, context.clone())
            .await
            .expect("list tools");
        let codemode_tool = tools
            .tools
            .iter()
            .find(|tool| tool.name.as_ref() == CODE_MODE_TOOL_NAME)
            .expect("Code Mode tool should be listed");
        let upstream_ui_tool = tools
            .tools
            .iter()
            .find(|tool| tool.name.as_ref() == UPSTREAM_UI_TOOL_NAME)
            .expect("upstream UI tool should be listed");
        assert!(
            codemode_tool
                .meta
                .as_ref()
                .and_then(|meta| meta.0["ui"]["resourceUri"].as_str())
                .is_some_and(|uri| uri.starts_with(CODE_MODE_APP_URI_PREFIX)),
            "Code Mode tool must keep its local UI resource: {codemode_tool:?}"
        );
        assert_eq!(
            upstream_ui_tool
                .meta
                .as_ref()
                .and_then(|meta| meta.0["ui"]["resourceUri"].as_str()),
            Some(UPSTREAM_UI_URI),
            "upstream UI tool must keep its native resource URI"
        );

        let resources = running
            .service()
            .list_resources_impl(None, context.clone())
            .await
            .expect("list resources");
        let uris = resources
            .resources
            .iter()
            .map(|resource| resource.uri.as_str())
            .collect::<Vec<_>>();
        assert!(
            uris.iter()
                .any(|uri| strip_app_version(uri) == CODE_MODE_APP_URI),
            "Code Mode app resource should be listed alongside upstream UI resources: {uris:?}"
        );
        assert!(
            uris.contains(&UPSTREAM_UI_URI),
            "upstream MCP UI resource should remain listed with its native URI: {uris:?}"
        );

        let code_mode_read = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new(CODE_MODE_APP_URI),
                context.clone(),
            )
            .await
            .expect("read Code Mode UI resource");
        let ResourceContents::TextResourceContents {
            text: code_mode_html,
            ..
        } = &code_mode_read.contents[0]
        else {
            panic!("expected Code Mode text resource");
        };
        assert!(code_mode_html.contains("Lab Code Mode Inspector"));

        let upstream_read = running
            .service()
            .read_resource_impl(ReadResourceRequestParams::new(UPSTREAM_UI_URI), context)
            .await
            .expect("read upstream UI resource");
        let ResourceContents::TextResourceContents {
            uri,
            text: upstream_html,
            ..
        } = &upstream_read.contents[0]
        else {
            panic!("expected upstream text resource");
        };
        assert_eq!(uri, UPSTREAM_UI_URI);
        assert!(upstream_html.contains("quick shell widget"));
    }

    #[tokio::test]
    async fn list_resources_only_lists_server_logs_app_for_admin_scope() {
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            code_mode_server().await,
            transport,
            None,
        );

        let denied = running
            .service()
            .list_resources_impl(None, scoped_context(running.peer().clone(), &["lab:read"]))
            .await
            .expect("list resources without admin scope");
        assert!(
            denied
                .resources
                .iter()
                .all(|resource| !resource.uri.starts_with(SERVER_LOGS_APP_URI_PREFIX)),
            "listed server logs UI resources without admin scope"
        );

        let allowed = running
            .service()
            .list_resources_impl(None, scoped_context(running.peer().clone(), &["lab:admin"]))
            .await
            .expect("list resources with admin scope");
        let server_logs_uris = allowed
            .resources
            .iter()
            .filter(|resource| resource.uri.starts_with(SERVER_LOGS_APP_URI_PREFIX))
            .map(|resource| resource.uri.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            server_logs_uris
                .iter()
                .map(|uri| strip_app_version(uri))
                .collect::<Vec<_>>(),
            vec![SERVER_LOGS_APP_URI]
        );
        assert!(
            server_logs_uris.iter().all(|uri| uri.contains("?v=")),
            "advertised server logs URI must carry a cache-bust token: {server_logs_uris:?}"
        );
    }

    #[tokio::test]
    async fn read_server_logs_app_resource_requires_admin_scope() {
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            code_mode_server().await,
            transport,
            None,
        );

        let err = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new(SERVER_LOGS_APP_URI),
                scoped_context(running.peer().clone(), &["lab:read"]),
            )
            .await
            .expect_err("server logs app resource must require admin");
        assert_eq!(
            err.data.as_ref().expect("error data")["kind"],
            json!("forbidden")
        );

        let ok = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new(SERVER_LOGS_APP_URI),
                scoped_context(running.peer().clone(), &["lab:admin"]),
            )
            .await
            .expect("server logs app resource with admin scope");
        let ResourceContents::TextResourceContents { text, .. } = &ok.contents[0] else {
            panic!("expected text resource");
        };
        assert!(text.contains("Server logs"));
        assert!(text.contains("server_logs.query"));
    }

    #[test]
    fn server_logs_app_html_exposes_log_viewer_affordances() {
        let html = server_logs_app_html(SERVER_LOGS_APP_URI).expect("server logs resource");

        for expected in [
            "LabbyServerLogs",
            "server_logs.query",
            "/v1/server-logs/query",
            "html.browser",
            "LabbyAppHost",
            "savedViews",
            "persistSavedViews",
            "requestSeq",
            "drillLinks",
            "Level",
            "Service",
            "Action",
            "Kind",
            "Search",
            "normalizeOutput",
            "value.ok===false&&value.error",
            "clearRows",
            "requestWidgetResize",
        ] {
            assert!(
                html.contains(expected),
                "server logs app must include marker `{expected}`"
            );
        }
    }

    #[test]
    fn server_logs_host_script_injection_fails_without_marker() {
        let descriptor = SERVER_LOGS_APP_RESOURCE_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.uri == SERVER_LOGS_APP_URI)
            .expect("server logs descriptor");

        let err = inline_app_host_script("<html></html>", descriptor)
            .expect_err("missing host script marker should fail");

        assert!(err.contains("missing Labby app host script marker"));
    }

    #[test]
    fn add_server_app_is_interactive_and_mobile_responsive() {
        let descriptor = ADD_SERVER_APP_RESOURCE_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.uri == ADD_SERVER_APP_URI)
            .expect("Add Server descriptor");
        let html = add_server_app()
            .inline_html(descriptor)
            .expect("Add Server HTML");

        for expected in [
            "Add Server",
            "Test Connection",
            "Create Server",
            "host.callAction(\"add_server\",action",
            "proxy_resources",
            "proxy_prompts",
            "@media (max-width:620px)",
            "env(safe-area-inset-bottom)",
            "min-height:48px",
            "ui/notifications/request-teardown",
            "probeStatus(result)",
            "result&&result.last_error",
            "lifecycle=\"closing\"",
            "observer.disconnect()",
            "originalButtonMarkup",
            "nameInput,targetInput,resources,prompts",
        ] {
            assert!(
                html.contains(expected),
                "Add Server app must include marker `{expected}`"
            );
        }
        assert!(html.contains("window.__LABBY_MCP_RESOURCE=true;"));
    }

    #[test]
    fn gateway_status_app_handles_live_status_and_mobile_lifecycle() {
        let descriptor = GATEWAY_STATUS_APP_RESOURCE_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.uri == GATEWAY_STATUS_APP_URI)
            .expect("Gateway Status descriptor");
        let html = gateway_status_app()
            .inline_html(descriptor)
            .expect("Gateway Status HTML");

        for expected in [
            "warning.message",
            "warnings.map",
            "exposed_tool_count??",
            "window.openai.toolOutput",
            "observer.disconnect()",
            ".badge.disabled",
            "min-height:44px",
            "visible ${plural",
            "showing data from",
        ] {
            assert!(
                html.contains(expected),
                "Gateway Status app must include marker `{expected}`"
            );
        }
        for forbidden in ["min-height:100dvh", "height+20", "text(warnings[0])"] {
            assert!(
                !html.contains(forbidden),
                "Gateway Status app must not include `{forbidden}`"
            );
        }
        assert!(html.contains("window.__LABBY_MCP_RESOURCE=true;"));
    }

    #[tokio::test]
    async fn gateway_status_resources_are_admin_only_and_use_runtime_mime() {
        let server = resource_scope_server(crate::mcp::route_scope::McpRouteScope::Root).await;
        let (transport, _client_transport) = tokio::io::duplex(64 * 1024);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );

        let denied = running
            .service()
            .list_resources_impl(None, scoped_context(running.peer().clone(), &["lab:read"]))
            .await
            .expect("read-scope resources");
        assert!(
            denied
                .resources
                .iter()
                .all(|resource| !resource.uri.starts_with(GATEWAY_STATUS_APP_URI))
        );

        let allowed = running
            .service()
            .list_resources_impl(None, scoped_context(running.peer().clone(), &["lab:admin"]))
            .await
            .expect("admin resources");
        assert!(
            allowed
                .resources
                .iter()
                .any(|resource| strip_app_version(&resource.uri) == GATEWAY_STATUS_APP_URI)
        );

        let forbidden = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new(GATEWAY_STATUS_APP_URI),
                scoped_context(running.peer().clone(), &["lab:read"]),
            )
            .await
            .expect_err("status resource must require admin");
        assert_eq!(
            forbidden.data.as_ref().expect("error data")["kind"],
            json!("forbidden")
        );

        for (uri, expected_mime) in [
            (GATEWAY_STATUS_APP_URI, CODE_MODE_APP_MIME),
            (
                GATEWAY_STATUS_APP_SKYBRIDGE_URI,
                CODE_MODE_APP_SKYBRIDGE_MIME,
            ),
        ] {
            let read = running
                .service()
                .read_resource_impl(
                    ReadResourceRequestParams::new(uri),
                    scoped_context(running.peer().clone(), &["lab:admin"]),
                )
                .await
                .expect("admin status resource");
            let ResourceContents::TextResourceContents {
                mime_type, text, ..
            } = &read.contents[0]
            else {
                panic!("expected text status resource");
            };
            assert_eq!(mime_type.as_deref(), Some(expected_mime));
            assert!(text.contains("Gateway Status"));
        }
    }

    #[tokio::test]
    async fn protected_scope_omits_disallowed_service_action_resources() {
        let server =
            resource_scope_server(crate::mcp::route_scope::McpRouteScope::protected_subset(
                "media",
                ["sonarr"],
                ["gateway"],
                false,
            ))
            .await;
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );

        let resources = running
            .service()
            .list_resources_impl(None, scoped_context(running.peer().clone(), &["lab:read"]))
            .await
            .expect("list resources");
        let uris = resources
            .resources
            .iter()
            .map(|resource| resource.uri.as_str())
            .collect::<Vec<_>>();

        assert!(
            uris.contains(&"lab://gateway/actions"),
            "allowed service action resource should be listed: {uris:?}"
        );
        assert!(
            !uris.contains(&"lab://deploy/actions"),
            "disallowed service action resource leaked into resources/list: {uris:?}"
        );
    }

    #[tokio::test]
    async fn list_resources_paginates_large_builtin_catalog() {
        let server = large_resource_server(250);
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );

        let first = running
            .service()
            .list_resources_impl(None, scoped_context(running.peer().clone(), &["lab:read"]))
            .await
            .expect("first page");

        assert_eq!(
            first.resources.len(),
            crate::mcp::pagination::MCP_LIST_PAGE_SIZE
        );
        assert_eq!(first.resources[0].uri, "lab://catalog");
        assert_eq!(first.next_cursor.as_deref(), Some("100"));
        let first_page_service_resources = first
            .resources
            .iter()
            .filter(|resource| resource.uri.starts_with("lab://resource_service_"))
            .count();
        assert!(
            first_page_service_resources > 0,
            "first page should include synthetic service resources"
        );

        let second_request =
            PaginatedRequestParams::default().with_cursor(first.next_cursor.clone());
        let second = running
            .service()
            .list_resources_impl(
                Some(second_request),
                scoped_context(running.peer().clone(), &["lab:read"]),
            )
            .await
            .expect("second page");

        let expected_first_service_on_second_page =
            format!("lab://resource_service_{first_page_service_resources:03}/actions");
        assert_eq!(
            second.resources[0].uri,
            expected_first_service_on_second_page
        );
    }

    #[tokio::test]
    async fn protected_scope_hides_code_mode_app_resources_when_disabled() {
        let server =
            resource_scope_server(crate::mcp::route_scope::McpRouteScope::protected_subset(
                "media",
                ["sonarr"],
                ["gateway"],
                false,
            ))
            .await;
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );

        let resources = running
            .service()
            .list_resources_impl(None, scoped_context(running.peer().clone(), &["lab:read"]))
            .await
            .expect("list resources");
        let uris = resources
            .resources
            .iter()
            .map(|resource| resource.uri.as_str())
            .collect::<Vec<_>>();

        assert!(
            uris.iter()
                .all(|uri| !uri.starts_with(CODE_MODE_APP_URI_PREFIX)),
            "Code Mode app resources leaked into resources/list with expose_code_mode=false: {uris:?}"
        );
    }

    #[tokio::test]
    async fn protected_scope_denies_code_mode_app_resource_read_when_disabled() {
        let server =
            resource_scope_server(crate::mcp::route_scope::McpRouteScope::protected_subset(
                "media",
                ["sonarr"],
                ["gateway"],
                false,
            ))
            .await;
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );

        for uri in [CODE_MODE_APP_URI, CODE_MODE_HISTORY_APP_URI] {
            let err = running
                .service()
                .read_resource_impl(
                    ReadResourceRequestParams::new(uri),
                    scoped_context(running.peer().clone(), &["lab:read"]),
                )
                .await
                .expect_err("Code Mode app resource must be hidden");

            assert!(
                err.message.contains("unknown UI resource"),
                "{uri} should be hidden as an unknown UI resource, got {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn protected_scope_denies_disallowed_service_action_resource_read() {
        let server =
            resource_scope_server(crate::mcp::route_scope::McpRouteScope::protected_subset(
                "media",
                ["sonarr"],
                ["gateway"],
                false,
            ))
            .await;
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );

        let err = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new("lab://deploy/actions"),
                scoped_context(running.peer().clone(), &["lab:read"]),
            )
            .await
            .expect_err("disallowed service action resource must be denied");

        assert_eq!(
            err.data.as_ref().expect("error data")["kind"],
            json!("route_scope_denied")
        );
    }

    #[tokio::test]
    async fn protected_scope_allows_allowed_service_action_resource_read() {
        let server =
            resource_scope_server(crate::mcp::route_scope::McpRouteScope::protected_subset(
                "media",
                ["sonarr"],
                ["gateway"],
                false,
            ))
            .await;
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );

        let allowed = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new("lab://gateway/actions"),
                scoped_context(running.peer().clone(), &["lab:read"]),
            )
            .await
            .expect("allowed service action resource");

        let ResourceContents::TextResourceContents { text, .. } = &allowed.contents[0] else {
            panic!("expected text resource");
        };
        assert!(
            text.contains(r#""name": "help""#),
            "allowed action resource should render the service action catalog: {text}"
        );
    }

    #[tokio::test]
    async fn read_history_resource_requires_read_scope_and_returns_html_metadata() {
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            code_mode_server().await,
            transport,
            None,
        );

        let denied = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new(CODE_MODE_HISTORY_APP_URI),
                scoped_context(running.peer().clone(), &["profile"]),
            )
            .await
            .expect_err("scope must be denied");
        assert_eq!(
            denied.data.as_ref().expect("error data")["kind"],
            json!("forbidden")
        );

        let allowed = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new(CODE_MODE_HISTORY_APP_URI),
                scoped_context(running.peer().clone(), &["lab:read"]),
            )
            .await
            .expect("read history resource");
        assert_eq!(allowed.contents.len(), 1);
        match &allowed.contents[0] {
            ResourceContents::TextResourceContents {
                uri,
                mime_type,
                text,
                meta,
            } => {
                assert_eq!(uri, CODE_MODE_HISTORY_APP_URI);
                assert_eq!(mime_type.as_deref(), Some(CODE_MODE_APP_MIME));
                assert!(text.contains("code_mode_history"));
                let meta = meta.as_ref().expect("resource metadata");
                assert_eq!(
                    meta.0["ui"]["resourceUri"],
                    json!(CODE_MODE_HISTORY_APP_URI)
                );
                assert_eq!(meta.0["ui"]["mimeTypes"], json!([CODE_MODE_APP_MIME]));
                assert_eq!(meta.0["ui"]["prefersBorder"], json!(false));
                assert_eq!(meta.0["ui"]["csp"]["connectDomains"], json!([]));
                assert!(meta.0.get("csp").is_none());
                assert!(meta.0.get("prefersBorder").is_none());
            }
            ResourceContents::BlobResourceContents { .. } => panic!("expected text resource"),
            _ => panic!("expected text resource"),
        }
    }

    #[tokio::test]
    async fn protected_scope_history_resource_hides_unscoped_entries() {
        let server =
            code_mode_server_with_scope(crate::mcp::route_scope::McpRouteScope::protected_subset(
                "media",
                ["sonarr"],
                ["gateway"],
                true,
            ))
            .await;
        let manager = server.gateway_manager.as_ref().expect("manager").clone();
        manager
            .record_code_mode_history(crate::dispatch::gateway::code_mode::CodeModeHistoryEntry {
                execution_id: None,
                seq: 0,
                route_scope: "root".to_string(),
                kind: crate::dispatch::gateway::code_mode::CodeModeHistoryKind::Execute,
                ok: true,
                elapsed_ms: 7,
                input_tokens: Some(3),
                output_tokens: Some(5),
                error_kind: None,
                calls: Vec::new(),
                match_count: None,
            })
            .await;
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );

        let allowed = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new(CODE_MODE_HISTORY_APP_URI),
                scoped_context(running.peer().clone(), &["lab:read"]),
            )
            .await
            .expect("read history resource");

        let ResourceContents::TextResourceContents { text, .. } = &allowed.contents[0] else {
            panic!("expected text resource");
        };
        assert!(
            text.contains(r#""entries":[]"#),
            "protected scope should not see global history: {text}"
        );
    }

    #[tokio::test]
    async fn skybridge_resource_is_readable_by_uri_despite_being_unlisted() {
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            code_mode_server().await,
            transport,
            None,
        );

        // OpenAI hosts never see this URI in resources/list (`listed: false`);
        // they reach it directly via the tool's `openai/outputTemplate`. Prove
        // the full read path serves it under the skybridge MIME with the
        // model-facing description attached.
        let allowed = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new(CODE_MODE_APP_SKYBRIDGE_URI),
                scoped_context(running.peer().clone(), &["lab:read"]),
            )
            .await
            .expect("read skybridge resource");
        let ResourceContents::TextResourceContents {
            uri,
            mime_type,
            text,
            meta,
        } = &allowed.contents[0]
        else {
            panic!("expected text resource");
        };
        assert_eq!(uri, CODE_MODE_APP_SKYBRIDGE_URI);
        assert_eq!(mime_type.as_deref(), Some(CODE_MODE_APP_SKYBRIDGE_MIME));
        assert!(text.contains("Lab Code Mode Inspector"));
        assert!(
            meta.as_ref()
                .expect("resource metadata")
                .0
                .contains_key("openai/widgetDescription")
        );

        // The unlisted resource still honors the read scope gate.
        let denied = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new(CODE_MODE_APP_SKYBRIDGE_URI),
                scoped_context(running.peer().clone(), &["profile"]),
            )
            .await
            .expect_err("scope must be denied");
        assert_eq!(
            denied.data.as_ref().expect("error data")["kind"],
            json!("forbidden")
        );
    }

    #[tokio::test]
    async fn unknown_code_mode_uri_is_rejected_by_the_read_path() {
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            code_mode_server().await,
            transport,
            None,
        );

        // The router admits any `ui://lab/code-mode/*` prefix; an un-tabled URI
        // under it must 404 through the full read path, not be served fallback HTML.
        let err = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new("ui://lab/code-mode/nope"),
                scoped_context(running.peer().clone(), &["lab:read"]),
            )
            .await
            .expect_err("un-tabled URI must be rejected");
        assert!(err.message.contains("unknown UI resource"), "{err:?}");
    }

    #[test]
    fn code_mode_app_descriptor_table_invariants_hold() {
        // MIME and listed-ness now derive from `runtime`, so the mime↔listed and
        // "both runtimes bound to one resource" failure modes are unrepresentable.
        // The one convention left is the tool↔descriptor mapping: every Code Mode
        // tool must have exactly one MCP (Claude) descriptor and exactly one
        // skybridge (OpenAI) descriptor, or it silently loses one runtime's binding.
        for tool in [CODE_MODE_TOOL_NAME] {
            assert_eq!(
                CODE_MODE_APP_RESOURCE_DESCRIPTORS
                    .iter()
                    .filter(|descriptor| {
                        descriptor.runtime == CodeModeRuntime::McpApp
                            && descriptor.tool_name == Some(tool)
                    })
                    .count(),
                1,
                "tool {tool} must have exactly one MCP (Claude) descriptor"
            );
            assert_eq!(
                CODE_MODE_APP_RESOURCE_DESCRIPTORS
                    .iter()
                    .filter(|descriptor| {
                        descriptor.runtime == CodeModeRuntime::Skybridge
                            && descriptor.tool_name == Some(tool)
                    })
                    .count(),
                1,
                "tool {tool} is missing its skybridge (OpenAI) descriptor"
            );
        }

        // Skybridge resources must never appear in resources/list (Claude surface).
        assert!(
            CODE_MODE_APP_RESOURCE_DESCRIPTORS
                .iter()
                .filter(|descriptor| descriptor.runtime == CodeModeRuntime::Skybridge)
                .all(|descriptor| !descriptor.runtime.listed()),
            "skybridge resources must stay out of resources/list"
        );

        // The one illegal state the enum can't prevent: a descriptor's URI must
        // match its runtime (a `.skybridge` URI on an McpApp row would be served
        // under the wrong MIME and leak into the Claude listing). Pin URI↔runtime.
        for descriptor in CODE_MODE_APP_RESOURCE_DESCRIPTORS {
            assert_eq!(
                descriptor.uri.ends_with(".skybridge"),
                descriptor.runtime == CodeModeRuntime::Skybridge,
                "descriptor {} URI suffix disagrees with its runtime",
                descriptor.uri
            );
        }

        // Lookups return None for an unmapped tool (the skybridge binding is then
        // silently omitted; the MCP binding `.expect()`s at the call site).
        assert_eq!(code_mode_app_resource_uri_for_tool("not_a_tool"), None);
        assert_eq!(code_mode_app_skybridge_uri_for_tool("not_a_tool"), None);
    }

    #[test]
    fn versioned_widget_uri_round_trips_through_the_read_path() {
        // The host fetches the advertised (versioned) URI. It must resolve to the
        // same descriptor/HTML as the base URI so the cache-bust token is purely a
        // cache key, not a new resource the read path can't find.
        let versioned = versioned_app_uri(CODE_MODE_APP_URI);
        assert!(versioned.starts_with(CODE_MODE_APP_URI));
        assert!(versioned.contains("?v="));
        assert_eq!(strip_app_version(&versioned), CODE_MODE_APP_URI);

        let from_base = code_mode_app_html(CODE_MODE_APP_URI, None).expect("base resource");
        let from_versioned = code_mode_app_html(&versioned, None).expect("versioned resource");
        assert_eq!(from_base, from_versioned);

        // Runtime/MIME resolution must also ignore the suffix.
        assert_eq!(
            code_mode_app_runtime_for_uri(&versioned).mime(),
            CODE_MODE_APP_MIME
        );

        // A base URI with no query is returned unchanged.
        assert_eq!(strip_app_version(CODE_MODE_APP_URI), CODE_MODE_APP_URI);

        // An un-tabled URI is still rejected even with a cache-bust suffix.
        let bogus = versioned_app_uri("ui://lab/code-mode/nope");
        assert!(code_mode_app_html(&bogus, None).is_err());
    }

    #[test]
    fn bridged_app_version_includes_the_injected_host_runtime() {
        let html = "<html>fixture</html>";
        let html_only = format!("{:016x}", fnv1a_64(html.as_bytes()));
        let combined = format!("{html}\n{}", crate::app_assets::LABBY_APP_HOST_JS);

        assert_eq!(
            bridged_app_content_version(html),
            format!("{:016x}", fnv1a_64(combined.as_bytes()))
        );
        assert_ne!(bridged_app_content_version(html), html_only);
    }

    #[test]
    fn add_server_resource_log_uri_redacts_query_credentials() {
        let uri = format!("{ADD_SERVER_APP_URI}?token=super-secret#fragment");
        let resource_uri_log =
            crate::dispatch::upstream::pool::redact_resource_uri_for_logging(&uri);

        assert_eq!(resource_uri_log, ADD_SERVER_APP_URI);
        assert!(!resource_uri_log.contains("super-secret"));
    }

    #[test]
    fn code_mode_app_html_accepts_known_ui_resources_and_rejects_unknown() {
        let html = code_mode_app_html(CODE_MODE_APP_URI, None).expect("known resource");
        assert!(html.contains("Lab Code Mode Inspector"));
        // The bundle hydrates natively under the OpenAI Apps runtime
        // (ChatGPT / Codex) via window.openai.toolOutput + openai:set_globals.
        // The bundle is hand-maintained vanilla JS with no JS harness, so these
        // string guards catch the regression where the whole OpenAI branch or its
        // "waiting" gate is dropped and only the React copy (which IS tested)
        // stays correct.
        assert!(
            html.contains("openai:set_globals"),
            "bundle must carry the OpenAI Apps hydration bridge"
        );
        assert!(
            html.contains("window.openai"),
            "bundle must branch on the OpenAI Apps runtime global"
        );
        assert!(
            html.contains("\"waiting\""),
            "bundle must keep the 'waiting' state so an empty widget isn't shown as connected"
        );

        // The skybridge variant serves the same HTML under the OpenAI MIME.
        let skybridge =
            code_mode_app_html(CODE_MODE_APP_SKYBRIDGE_URI, None).expect("skybridge resource");
        assert!(skybridge.contains("Lab Code Mode Inspector"));

        let err = code_mode_app_html("ui://lab/code-mode/nope", None).expect_err("unknown");
        assert!(err.contains("unknown UI resource"));
    }

    #[test]
    fn skybridge_and_mcp_app_resource_meta_diverge_by_runtime() {
        // OpenAI skybridge resource: skybridge MIME + model-facing description.
        let skybridge = code_mode_app_resource_meta(CODE_MODE_APP_SKYBRIDGE_URI);
        assert_eq!(
            skybridge.0["ui"]["mimeTypes"][0].as_str(),
            Some(CODE_MODE_APP_SKYBRIDGE_MIME)
        );
        assert!(
            skybridge.0.contains_key("openai/widgetDescription"),
            "skybridge resource must carry an OpenAI widget description"
        );

        // Claude resource: MCP Apps MIME, and byte-identical (no openai/* keys).
        let mcp_app = code_mode_app_resource_meta(CODE_MODE_APP_URI);
        assert_eq!(
            mcp_app.0["ui"]["mimeTypes"][0].as_str(),
            Some(CODE_MODE_APP_MIME)
        );
        assert!(
            !mcp_app.0.contains_key("openai/widgetDescription"),
            "Claude resource _meta must stay free of OpenAI compatibility keys"
        );
    }

    #[test]
    fn code_mode_app_resources_follow_synthetic_tool_visibility() {
        let read_auth = labby_auth::auth_context::AuthContext {
            sub: "reader".to_string(),
            actor_key: None,
            scopes: vec!["lab:read".to_string()],
            issuer: "https://lab.example.com".to_string(),
            via_session: true,
            csrf_token: None,
            email: None,
        };
        let denied_auth = labby_auth::auth_context::AuthContext {
            scopes: vec!["profile".to_string()],
            ..read_auth.clone()
        };
        assert!(
            code_mode_app_resources_visible(true, Some(&read_auth)),
            "Code Mode app resources should be listed with the synthetic codemode tool"
        );
        assert!(
            !code_mode_app_resources_visible(true, Some(&denied_auth)),
            "Code Mode app resources should not be listed without Code Mode read scope"
        );
        assert!(
            !code_mode_app_resources_visible(false, Some(&read_auth)),
            "Code Mode app resources should not be listed when synthetic tools are disabled"
        );
        let resources = code_mode_app_resources();
        let uris = resources
            .iter()
            .map(|resource| strip_app_version(&resource.uri).to_string())
            .collect::<Vec<_>>();
        assert_eq!(uris, vec![CODE_MODE_APP_URI, CODE_MODE_HISTORY_APP_URI]);
        // The tool-binding URI carries the cache-bust token but resolves to the
        // canonical base after stripping it.
        let codemode_uri =
            code_mode_app_resource_uri_for_tool(CODE_MODE_TOOL_NAME).expect("codemode uri");
        assert!(codemode_uri.contains("?v="));
        assert_eq!(strip_app_version(&codemode_uri), CODE_MODE_APP_URI);
    }

    #[test]
    fn code_mode_history_html_injects_escaped_snapshot() {
        let html = code_mode_app_html(
            CODE_MODE_HISTORY_APP_URI,
            Some(&json!({
                "kind": "code_mode_history",
                "entries": [{"seq": 1, "kind": "execute", "ok": true, "elapsed_ms": 1, "calls": [{"params": {"note": "</script>"}}]}],
            })),
        )
        .expect("history resource");

        assert!(html.contains("code_mode_history"));
        assert!(!html.contains("</script>\""));
        assert!(html.contains("\\u003c/script>"));
    }

    #[test]
    fn code_mode_app_html_uses_current_trace_field_names() {
        let html = code_mode_app_html(
            CODE_MODE_APP_URI,
            Some(&json!({
                "kind": "code_mode_execute_trace",
                "call_count": 1,
                "calls": [{
                    "id": "github::search_issues",
                    "upstream": "github",
                    "tool": "search_issues",
                    "ok": true,
                    "elapsed_ms": 12,
                    "ui": {"resourceUri": "ui://github/search.html"},
                    "result_shape": {"type": "array", "length": 3},
                }],
            })),
        )
        .expect("codemode resource");

        assert!(html.contains("call.ok"));
        assert!(html.contains("call.error_kind"));
        assert!(html.contains("call.ui"));
        assert!(html.contains("resourceUri"));
        assert!(html.contains("MCP UI"));
        assert!(html.contains("s.length"));
        assert!(
            html.contains("call.namespace"),
            "inline app must read the emitted namespace field (with an id-split fallback)"
        );
        assert!(
            !html.contains("call.status"),
            "inline app must use the emitted ok boolean, not stale status fields"
        );
        assert!(
            !html.contains("array_len"),
            "inline app must use result_shape.length"
        );
    }

    #[test]
    fn code_mode_app_html_gates_connected_state_on_bridge_handshake() {
        let html = code_mode_app_html(CODE_MODE_APP_URI, None).expect("codemode resource");
        // Status must not be claimed "connected" before the bridge resolves.
        assert!(
            html.contains("\"connecting\""),
            "MCP Apps branch must start from a 'connecting' state, not optimistic 'connected'"
        );
        assert!(
            html.contains("if (!hydrated) setState(\"connected\", true)"),
            "MCP Apps branch must gate 'connected' on the connect() handshake"
        );
    }

    #[test]
    fn code_mode_app_html_preserves_expanded_rows_and_uses_available_width() {
        let html = code_mode_app_html(CODE_MODE_APP_URI, None).expect("codemode resource");

        assert!(
            html.contains("data-row-key"),
            "rows need stable keys so repaint can preserve expanded state"
        );
        assert!(
            html.contains("snapshotExpandedRows"),
            "paint must snapshot expanded rows before replacing the DOM"
        );
        assert!(
            html.contains("restoreExpandedRows"),
            "paint must restore expanded rows after replacing the DOM"
        );
        assert!(
            html.contains("max-width:none"),
            "the ChatGPT app should use the host-provided width"
        );
        assert!(
            !html.contains("max-width:680px"),
            "the old 680px cap leaves unused space around the inspector"
        );
    }

    #[test]
    fn code_mode_app_html_reports_content_sized_height() {
        let html = code_mode_app_html(CODE_MODE_APP_URI, None).expect("codemode resource");

        assert!(
            html.contains("function scheduleResize"),
            "inline app should explicitly measure its rendered widget height"
        );
        assert!(
            html.contains("sendSizeChanged"),
            "inline app should notify MCP Apps hosts when content height changes"
        );
        let reset_height = html
            .find("document.documentElement.style.height=\"auto\"")
            .expect("inline app resets the persisted root height");
        let measure_height = html
            .find("document.body.getBoundingClientRect()")
            .expect("inline app measures its content height");
        assert!(
            reset_height < measure_height,
            "persisted heights must be reset before measuring so the app can shrink"
        );
        assert!(
            html.contains("if(activeMcpUiUri!==uri)setMinimized(true)"),
            "repainting the same MCP UI must preserve a user's restored inspector state"
        );
        assert!(
            html.contains("autoResize: false"),
            "document-root auto-resize can over-report empty iframe space below the widget"
        );
    }

    #[test]
    fn code_mode_app_html_exposes_debugger_ui_affordances() {
        let html = code_mode_app_html(CODE_MODE_APP_URI, None).expect("codemode resource");

        for expected in [
            "summaryStats",
            "section(\"Calls\"",
            "section(\"Request\"",
            "section(\"Response\"",
            "row-error",
            "viewTab(\"pretty\"",
            "viewTab(\"raw\"",
            "viewTab(\"shape\"",
            "rowcopy",
            "longest",
            "Run ",
            "border-radius:10px",
        ] {
            assert!(
                html.contains(expected),
                "inline app must include debugger UI affordance marker `{expected}`"
            );
        }
    }

    #[test]
    fn code_mode_app_html_surfaces_action_dispatched_calls() {
        let html = code_mode_app_html(CODE_MODE_APP_URI, None).expect("codemode resource");

        assert!(
            html.contains("function callActionLabel"),
            "inline app must derive a readable action label from call params"
        );
        assert!(
            html.contains("params.action"),
            "action-dispatched one-tool servers should show params.action"
        );
        assert!(
            html.contains("action-label"),
            "call rows should render the derived action label separately from the tool id"
        );
    }

    #[test]
    fn code_mode_app_html_exposes_inspector_power_tools() {
        let html = code_mode_app_html(CODE_MODE_APP_URI, None).expect("codemode resource");

        for expected in [
            "copyReplaySnippet",
            "saveSnippet",
            "resultSearch",
            "setAllRows",
            "actionDescription",
            "callInvocationMode",
            "emptyReason",
            "truncationNotice",
            "historyDelta",
        ] {
            assert!(
                html.contains(expected),
                "inline app must include inspector power-tool marker `{expected}`"
            );
        }
    }
}
