//! Completion + per-service action-schema helpers for the MCP server.
//!
//! Pure free functions extracted from `server.rs` (bead `lab-kvji.24.1.1`).
//! No behavior change — relocation + `pub(crate)` visibility only.

use std::time::Instant;

use rmcp::model::{CompleteRequestParams, CompleteResult, CompletionInfo};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use serde_json::Value;

use crate::catalog::Catalog;

/// JSON Schema for every service tool's input: `action` (required) + `params` (optional object).
#[allow(clippy::expect_used)]
pub(crate) fn action_schema() -> serde_json::Map<String, Value> {
    serde_json::json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "description": "Action to perform (e.g. \"status.get\"). Use \"help\" to list all actions."
            },
            "params": {
                "type": "object",
                "description": "Action-specific parameters (varies per action)"
            }
        },
        "required": ["action"]
    })
    .as_object()
    .cloned()
    .expect("schema literal is always an object")
}

pub(crate) fn completion_info(values: Vec<String>) -> CompletionInfo {
    let total = values.len() as u32;
    let has_more = values.len() > CompletionInfo::MAX_VALUES;
    let values = values
        .into_iter()
        .take(CompletionInfo::MAX_VALUES)
        .collect();
    CompletionInfo::with_pagination(values, Some(total), has_more)
        .expect("completion values are capped at rmcp's maximum")
}

pub(crate) fn complete_prompt_arg(
    catalog: &Catalog,
    prompt_name: &str,
    argument_name: &str,
    prefix: &str,
) -> CompletionInfo {
    let mut names: Vec<String> = match (prompt_name, argument_name) {
        ("run-action", "action") => catalog
            .services
            .iter()
            .flat_map(|service| &service.actions)
            .filter(|action| action.name.starts_with(prefix))
            .map(|action| action.name.clone())
            .collect(),
        ("run-action" | "service-discover", "service") => catalog
            .services
            .iter()
            .filter(|service| service.name.starts_with(prefix))
            .map(|service| service.name.clone())
            .collect(),
        _ => Vec::new(),
    };
    names.sort_unstable();
    names.dedup();
    completion_info(names)
}

#[cfg(feature = "gateway")]
use crate::mcp::context::{auth_context_from_extensions, oauth_upstream_subject_for_request};
use crate::mcp::logging::{DispatchLogOutcome, LoggingLevel};
use crate::mcp::server::LabMcpServer;

/// Revalidate AssetUse and the exact catalog around the upstream RPC.
#[cfg(feature = "gateway")]
pub(crate) async fn complete_project_reference(
    runtime: &crate::access::AccessRuntime,
    manager: &labby_gateway::gateway::manager::GatewayManager,
    transport: &crate::mcp::bound_access::TransportBoundAccessContext,
    identity: &labby_auth::VerifiedIdentity,
    mut request: CompleteRequestParams,
) -> Option<Result<CompleteResult, labby_gateway::upstream::pool::CapabilityCallError>> {
    use crate::mcp::bound_access::bind_asset_use_access_context;
    use labby_gateway::upstream::pool::CompletionCatalogGeneration;
    use std::time::SystemTime;
    let route = transport.core().route();
    let bind = || {
        bind_asset_use_access_context(
            runtime,
            manager,
            identity.clone(),
            route.route_name(),
            route.resource(),
            route.project_id(),
        )
    };
    if !transport.matches_identity(identity) {
        return None;
    }
    let before = bind().await.ok()?;
    if !before.authorizes_from(transport.core(), crate::access::Permission::AssetUse) {
        return None;
    }
    transport.validate_not_expired(SystemTime::now()).ok()?;
    let catalog = before.catalog().catalog();
    let (upstream, generation, pool_generation) = match &mut request.r#ref {
        rmcp::model::Reference::Prompt(reference) => {
            let prompts = catalog.prompts();
            let published = prompts.unique_route_for_wire_name(&reference.name)?;
            if !before.allows_upstream_prompt_pair(&published.upstream_name, &published.native_name)
            {
                return None;
            }
            reference.name = published.native_name.to_string();
            (
                published.upstream_name.to_string(),
                CompletionCatalogGeneration::Prompt(prompts.prompt_catalog_generation()),
                prompts.pool_publication_generation(),
            )
        }
        rmcp::model::Reference::Resource(reference) => {
            let templates = catalog.resource_templates();
            let (upstream, native) = reference
                .uri
                .strip_prefix("lab://upstream/")?
                .split_once('/')?;
            if !before.allows_upstream_resource_template_pair(upstream, native) {
                return None;
            }
            let upstream = upstream.to_owned();
            reference.uri = native.to_owned();
            (
                upstream,
                CompletionCatalogGeneration::ResourceTemplate(
                    templates.resource_template_catalog_generation(),
                ),
                templates.pool_publication_generation(),
            )
        }
        _ => return None,
    };
    let pool = manager.current_pool_sync()?;
    if manager.current_pool_publication_generation() != pool_generation {
        return None;
    }
    let result = pool
        .complete_published_reference(&upstream, generation, request)
        .await;
    let after = bind().await.ok()?;
    transport.validate_not_expired(SystemTime::now()).ok()?;
    before
        .same_publication_as(&after)
        .then_some(result)
        .flatten()
}

impl LabMcpServer {
    pub(crate) async fn complete_impl(
        &self,
        request: CompleteRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, ErrorData> {
        let start = Instant::now();
        let subject = self.request_subject_log_tag(&context);
        let reference_type = request.r#ref.reference_type();
        let prompt_name = request.r#ref.as_prompt_name().map(str::to_string);

        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "completion.complete",
            subject,
            reference_type,
            prompt = prompt_name.as_deref().unwrap_or(""),
            argument = %request.argument.name,
            "dispatch start"
        );

        if (prompt_name.is_some() && !self.route_scope.exposes_prompts())
            || (request.r#ref.as_resource_uri().is_some() && !self.route_scope.exposes_resources())
        {
            return Ok(CompleteResult::new(completion_info(Vec::new())));
        }
        #[cfg(feature = "gateway")]
        match crate::mcp::bound_access::project_execution_binding(
            &context.extensions,
            std::time::SystemTime::now(),
        ) {
            crate::mcp::bound_access::ProjectExecutionBinding::Legacy => {}
            crate::mcp::bound_access::ProjectExecutionBinding::Unavailable => {
                return Ok(CompleteResult::new(completion_info(Vec::new())));
            }
            crate::mcp::bound_access::ProjectExecutionBinding::Bound {
                transport,
                identity,
            } => {
                let result = match &self.gateway_manager {
                    Some(manager) => {
                        complete_project_reference(
                            &self.access_runtime,
                            manager,
                            transport,
                            identity,
                            request,
                        )
                        .await
                    }
                    None => None,
                };
                return match result {
                    Some(Ok(result)) => Ok(result),
                    Some(Err(error)) => {
                        let (kind, _) = crate::mcp::resource_errors::classify_fetch_failure(&error);
                        Err(crate::mcp::agent_error::internal(
                            kind,
                            "Upstream completion failed",
                            None,
                            &labby_runtime::agent_error::AgentErrorContext::for_service_action(
                                "labby",
                                "completion.complete",
                            ),
                        ))
                    }
                    None => Ok(CompleteResult::new(completion_info(Vec::new()))),
                };
            }
        }
        let local_prompt = prompt_name
            .as_deref()
            .is_some_and(|name| matches!(name, "run-action" | "service-discover"));
        if local_prompt {
            let completion = complete_prompt_arg(
                &self.visible_catalog().await,
                prompt_name.as_deref().expect("known local prompt"),
                &request.argument.name,
                &request.argument.value,
            );
            let elapsed_ms = start.elapsed().as_millis();
            self.trace_dispatch_outcome(
                &context,
                "lab",
                "completion.complete",
                elapsed_ms,
                DispatchLogOutcome::Success,
            );
            return Ok(CompleteResult::new(completion));
        }

        #[cfg(feature = "gateway")]
        if let Some(pool) = self.current_upstream_pool().await {
            let auth = auth_context_from_extensions(&context.extensions);
            let oauth_subject = self.route_oauth_subject(oauth_upstream_subject_for_request(
                auth,
                self.request_subject(&context),
            ));

            if let Some(oauth_subject) = oauth_subject.as_deref() {
                let configs = self.route_scoped_oauth_upstream_configs().await;
                let oauth_owner = if let Some(resource_uri) = request.r#ref.as_resource_uri() {
                    resource_uri
                        .strip_prefix("lab://upstream/")
                        .and_then(|rest| rest.split('/').next())
                        .and_then(|owner| {
                            configs
                                .iter()
                                .find(|config| config.name == owner && config.oauth.is_some())
                        })
                        .cloned()
                } else if let Some(prompt_name) = request.r#ref.as_prompt_name() {
                    // The gateway namespace identifies the owner. The pool's
                    // completion path enforces exposure before acquiring a peer.
                    prompt_name
                        .split_once('/')
                        .and_then(|(owner, _)| {
                            configs
                                .iter()
                                .find(|config| config.name == owner && config.oauth.is_some())
                        })
                        .cloned()
                } else {
                    None
                };

                if let Some(config) = oauth_owner {
                    return pool
                        .subject_scoped_complete_reference(&config, oauth_subject, request)
                        .await
                        .map_err(|message| ErrorData::internal_error(message, None));
                }
            }

            let raw_owner = if let Some(resource_uri) = request.r#ref.as_resource_uri() {
                resource_uri
                    .strip_prefix("lab://upstream/")
                    .and_then(|rest| rest.split('/').next())
                    .filter(|owner| self.route_scope.allows_upstream(owner))
                    .map(str::to_string)
            } else if let Some(prompt_name) = request.r#ref.as_prompt_name() {
                pool.find_prompt_owner_allowed(prompt_name, self.route_scope.allowed_upstreams())
                    .await
            } else {
                None
            };

            if let Some(owner) = raw_owner
                && self.route_team_credential_valid(&owner).await
            {
                match pool.complete_reference(&owner, request).await {
                    Some(Ok(result)) => return Ok(result),
                    Some(Err(message)) => {
                        let elapsed_ms = start.elapsed().as_millis();
                        self.trace_dispatch_outcome(
                            &context,
                            "lab",
                            "completion.complete",
                            elapsed_ms,
                            DispatchLogOutcome::Failure {
                                level: LoggingLevel::Error,
                                kind: "upstream_error".into(),
                            },
                        );
                        return Err(ErrorData::internal_error(message, None));
                    }
                    None => {}
                }
            }
        }

        let completion = completion_info(Vec::new());
        let elapsed_ms = start.elapsed().as_millis();
        self.trace_dispatch_outcome(
            &context,
            "lab",
            "completion.complete",
            elapsed_ms,
            DispatchLogOutcome::Success,
        );
        Ok(CompleteResult::new(completion))
    }
}

#[cfg(test)]
mod tests;
