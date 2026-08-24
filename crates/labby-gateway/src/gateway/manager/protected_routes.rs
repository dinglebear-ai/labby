//! Manager-level protected-route management: live resolver lookups plus CRUD
//! that keeps the in-memory route index in sync with persisted config.

use crate::gateway::config::{
    insert_protected_mcp_route, remove_protected_mcp_route, update_protected_mcp_route,
};
use labby_runtime::error::ToolError;
use labby_runtime::gateway_config::{
    GatewayConfig, ProtectedMcpRouteConfig, ProtectedMcpRouteTarget,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

use super::GatewayManager;

impl GatewayManager {
    pub async fn resolve_protected_route(
        &self,
        host: &str,
        path: &str,
    ) -> Option<ProtectedMcpRouteConfig> {
        let _publication = self.publication_barrier.read().await;
        self.protected_route_index.read().await.resolve(host, path)
    }

    pub async fn resolve_protected_route_metadata(
        &self,
        host: &str,
        path: &str,
    ) -> Option<ProtectedMcpRouteConfig> {
        let _publication = self.publication_barrier.read().await;
        self.protected_route_index
            .read()
            .await
            .resolve_exact_metadata_path(host, path)
    }

    pub async fn protected_route_list(&self) -> Vec<ProtectedMcpRouteConfig> {
        self.config.read().await.protected_mcp_routes.clone()
    }

    /// Desired protected-route config compared with the routes this process
    /// actually booted/published. Staged gateway-subset mutations deliberately
    /// write durable config without changing `self.config` or the live route
    /// index, so this view is the control-plane source of truth for pending
    /// restart work.
    pub async fn protected_route_list_state(&self) -> Result<Vec<Value>, ToolError> {
        let desired_cfg = self.load_config_for_mutation().await?;
        let runtime_cfg = self.config.read().await.clone();
        let global_restart_required = super::config_transaction::protected_routes_have_restart_debt(
            &runtime_cfg,
            &desired_cfg,
        );
        let desired = desired_cfg.protected_mcp_routes;
        let runtime = runtime_cfg.protected_mcp_routes;
        let names = desired
            .iter()
            .chain(runtime.iter())
            .map(|route| route.name.clone())
            .collect::<BTreeSet<_>>();
        let mut rows = Vec::with_capacity(names.len());
        for name in names {
            let desired_route = desired.iter().find(|route| route.name == name);
            let runtime_route = runtime.iter().find(|route| route.name == name);
            let changed = desired_route != runtime_route;
            let restart_required = changed && global_restart_required;
            let pending_operation = if !restart_required {
                None
            } else if runtime_route.is_none() {
                Some("add")
            } else if desired_route.is_none() {
                Some("remove")
            } else {
                Some("update")
            };
            let display = desired_route
                .or(runtime_route)
                .expect("name came from one route set");
            let mut value = serde_json::to_value(display).map_err(|error| {
                ToolError::internal_message(format!(
                    "failed to serialize protected route state: {error}"
                ))
            })?;
            let object = value.as_object_mut().ok_or_else(|| {
                ToolError::internal_message("protected route state did not serialize as an object")
            })?;
            object.insert(
                "restart_required".to_string(),
                Value::Bool(restart_required),
            );
            object.insert(
                "pending_operation".to_string(),
                pending_operation.map_or(Value::Null, |operation| {
                    Value::String(operation.to_string())
                }),
            );
            object.insert(
                "runtime_present".to_string(),
                Value::Bool(runtime_route.is_some()),
            );
            object.insert(
                "desired_present".to_string(),
                Value::Bool(desired_route.is_some()),
            );
            rows.push(value);
        }
        Ok(rows)
    }

    pub async fn protected_route_get(
        &self,
        name: &str,
    ) -> Result<ProtectedMcpRouteConfig, ToolError> {
        self.load_config_for_mutation()
            .await?
            .protected_mcp_routes
            .into_iter()
            .find(|route| route.name == name)
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("protected MCP route `{name}` not found in desired config"),
            })
    }

    pub async fn protected_route_add(
        &self,
        route: ProtectedMcpRouteConfig,
    ) -> Result<ProtectedMcpRouteConfig, ToolError> {
        reject_hot_gateway_subset_mutation(&route, "add")?;
        let _mutation_guard = self.acquire_config_mutation().await?;
        let mut cfg = self.load_config_for_mutation().await?;
        let runtime_cfg = self.config.read().await.clone();
        reject_pending_route_restart(&runtime_cfg, &cfg, "add")?;
        if let Some(runtime_existing) = runtime_cfg
            .protected_mcp_routes
            .iter()
            .find(|existing| existing.name == route.name)
        {
            reject_hot_gateway_subset_mutation(runtime_existing, "add")?;
        }
        let route = insert_protected_mcp_route(&mut cfg, route)?;
        self.persist_config_owned(_mutation_guard, cfg).await?;
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.protected_route.add",
            route = %route.name,
            public_host = %route.public_host,
            public_path = %route.public_path,
            upstream = ?route.upstream,
            backend_url = %route.backend_url,
            backend_mcp_path = %route.backend_mcp_path,
            enabled = route.enabled,
            scopes = ?route.scopes,
            "protected MCP route added"
        );
        Ok(route)
    }

    pub async fn protected_route_update(
        &self,
        name: &str,
        mut route: ProtectedMcpRouteConfig,
        preserve_project_id: bool,
    ) -> Result<ProtectedMcpRouteConfig, ToolError> {
        let _mutation_guard = self.acquire_config_mutation().await?;
        let mut cfg = self.load_config_for_mutation().await?;
        let runtime_cfg = self.config.read().await.clone();
        if preserve_project_id {
            preserve_route_project_id(&cfg, name, &mut route)?;
        }
        reject_pending_route_restart(&runtime_cfg, &cfg, "update")?;
        if let Some(existing) = cfg
            .protected_mcp_routes
            .iter()
            .find(|route| route.name == name)
        {
            reject_hot_gateway_subset_mutation(existing, "update")?;
        }
        if let Some(runtime_existing) = runtime_cfg
            .protected_mcp_routes
            .iter()
            .find(|existing| existing.name == name)
        {
            reject_hot_gateway_subset_mutation(runtime_existing, "update")?;
        }
        reject_hot_gateway_subset_mutation(&route, "update")?;
        let route = update_protected_mcp_route(&mut cfg, name, route)?;
        self.persist_config_owned(_mutation_guard, cfg).await?;
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.protected_route.update",
            route = %route.name,
            previous_name = %name,
            public_host = %route.public_host,
            public_path = %route.public_path,
            upstream = ?route.upstream,
            backend_url = %route.backend_url,
            backend_mcp_path = %route.backend_mcp_path,
            enabled = route.enabled,
            scopes = ?route.scopes,
            "protected MCP route updated"
        );
        Ok(route)
    }

    pub async fn protected_route_remove(
        &self,
        name: &str,
    ) -> Result<ProtectedMcpRouteConfig, ToolError> {
        let _mutation_guard = self.acquire_config_mutation().await?;
        let mut cfg = self.load_config_for_mutation().await?;
        let runtime_cfg = self.config.read().await.clone();
        reject_pending_route_restart(&runtime_cfg, &cfg, "remove")?;
        if let Some(existing) = cfg
            .protected_mcp_routes
            .iter()
            .find(|route| route.name == name)
        {
            reject_hot_gateway_subset_mutation(existing, "remove")?;
        }
        if let Some(runtime_existing) = runtime_cfg
            .protected_mcp_routes
            .iter()
            .find(|existing| existing.name == name)
        {
            reject_hot_gateway_subset_mutation(runtime_existing, "remove")?;
        }
        let route = remove_protected_mcp_route(&mut cfg, name)?;
        self.persist_config_owned(_mutation_guard, cfg).await?;
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.protected_route.remove",
            route = %route.name,
            public_host = %route.public_host,
            public_path = %route.public_path,
            upstream = ?route.upstream,
            backend_url = %route.backend_url,
            backend_mcp_path = %route.backend_mcp_path,
            "protected MCP route removed"
        );
        Ok(route)
    }

    pub async fn protected_route_stage_add(
        &self,
        route: ProtectedMcpRouteConfig,
    ) -> Result<Value, ToolError> {
        let started = std::time::Instant::now();
        let _mutation_guard = self.acquire_config_mutation().await?;
        let mut cfg = self.load_config_for_mutation().await?;
        let runtime_cfg = self.config.read().await.clone();
        let runtime_existing = runtime_cfg
            .protected_mcp_routes
            .iter()
            .find(|existing| existing.name == route.name)
            .cloned();
        let pending_restart =
            super::config_transaction::protected_routes_have_restart_debt(&runtime_cfg, &cfg);
        if !pending_restart
            && !route.is_gateway_subset()
            && !runtime_existing
                .as_ref()
                .is_some_and(ProtectedMcpRouteConfig::is_gateway_subset)
        {
            return Err(ToolError::InvalidParam {
                message: "staging is only needed for a gateway_subset desired route or when replacing a still-mounted gateway_subset route; use gateway.protected_route.add for a directly hot-reloadable backend route".to_string(),
                param: "route.target".to_string(),
            });
        }
        let route = insert_protected_mcp_route(&mut cfg, route)?;
        let runtime_result = runtime_cfg
            .protected_mcp_routes
            .iter()
            .find(|runtime| runtime.name == route.name);
        let restart_required =
            super::config_transaction::protected_routes_have_restart_debt(&runtime_cfg, &cfg);
        let result = staged_route_result(
            route.clone(),
            Some(&route),
            runtime_result,
            restart_required,
        );
        let restart_required = result["restart_required"].as_bool().unwrap_or(true);
        if restart_required {
            self.persist_desired_config_owned(_mutation_guard, cfg)
                .await?;
        } else {
            // Once the last gateway-subset restart dependency is gone, every
            // remaining desired route difference is hot-safe. Publish that
            // accumulated direct-route state now so a no-restart result can
            // never get ahead of the live route index.
            self.persist_config_owned(_mutation_guard, cfg).await?;
        }
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.protected_route.stage_add",
            route = %route.name,
            public_host = %route.public_host,
            public_path = %route.public_path,
            elapsed_ms = started.elapsed().as_millis(),
            restart_required,
            "protected MCP route desired state saved"
        );
        Ok(result)
    }

    pub async fn protected_route_stage_update(
        &self,
        name: &str,
        mut route: ProtectedMcpRouteConfig,
        preserve_project_id: bool,
    ) -> Result<Value, ToolError> {
        let started = std::time::Instant::now();
        let _mutation_guard = self.acquire_config_mutation().await?;
        let mut cfg = self.load_config_for_mutation().await?;
        let desired_existing = cfg
            .protected_mcp_routes
            .iter()
            .find(|existing| existing.name == name)
            .cloned();
        if preserve_project_id {
            preserve_route_project_id(&cfg, name, &mut route)?;
        }
        let runtime_cfg = self.config.read().await.clone();
        let runtime_existing = runtime_cfg
            .protected_mcp_routes
            .iter()
            .find(|existing| existing.name == name)
            .cloned();
        let pending_restart =
            super::config_transaction::protected_routes_have_restart_debt(&runtime_cfg, &cfg);
        let subset_related = route.is_gateway_subset()
            || desired_existing
                .as_ref()
                .is_some_and(ProtectedMcpRouteConfig::is_gateway_subset)
            || runtime_existing
                .as_ref()
                .is_some_and(ProtectedMcpRouteConfig::is_gateway_subset);
        if !subset_related && !pending_restart {
            return Err(ToolError::InvalidParam {
                message: "staging is only needed when the current or replacement route is a gateway_subset; use gateway.protected_route.update for a directly hot-reloadable backend route".to_string(),
                param: "route.target".to_string(),
            });
        }
        let route = update_protected_mcp_route(&mut cfg, name, route)?;
        let runtime_result = runtime_cfg
            .protected_mcp_routes
            .iter()
            .find(|runtime| runtime.name == route.name);
        let restart_required =
            super::config_transaction::protected_routes_have_restart_debt(&runtime_cfg, &cfg);
        let result = staged_route_result(
            route.clone(),
            Some(&route),
            runtime_result,
            restart_required,
        );
        let restart_required = result["restart_required"].as_bool().unwrap_or(true);
        if restart_required {
            self.persist_desired_config_owned(_mutation_guard, cfg)
                .await?;
        } else {
            // Once the last gateway-subset restart dependency is gone, every
            // remaining desired route difference is hot-safe. Publish that
            // accumulated direct-route state now so a no-restart result can
            // never get ahead of the live route index.
            self.persist_config_owned(_mutation_guard, cfg).await?;
        }
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.protected_route.stage_update",
            route = %route.name,
            previous_name = %name,
            public_host = %route.public_host,
            public_path = %route.public_path,
            elapsed_ms = started.elapsed().as_millis(),
            restart_required,
            "protected MCP route desired update saved"
        );
        Ok(result)
    }

    pub async fn protected_route_stage_remove(&self, name: &str) -> Result<Value, ToolError> {
        let started = std::time::Instant::now();
        let _mutation_guard = self.acquire_config_mutation().await?;
        let mut cfg = self.load_config_for_mutation().await?;
        let desired_existing = cfg
            .protected_mcp_routes
            .iter()
            .find(|existing| existing.name == name)
            .cloned();
        let runtime_cfg = self.config.read().await.clone();
        let runtime_existing = runtime_cfg
            .protected_mcp_routes
            .iter()
            .find(|existing| existing.name == name)
            .cloned();
        let pending_restart =
            super::config_transaction::protected_routes_have_restart_debt(&runtime_cfg, &cfg);
        let subset_related = desired_existing
            .as_ref()
            .is_some_and(ProtectedMcpRouteConfig::is_gateway_subset)
            || runtime_existing
                .as_ref()
                .is_some_and(ProtectedMcpRouteConfig::is_gateway_subset);
        if !subset_related && !pending_restart {
            return Err(ToolError::InvalidParam {
                message: "staging is only needed for a gateway_subset protected route; use gateway.protected_route.remove for a directly hot-reloadable backend route".to_string(),
                param: "name".to_string(),
            });
        }
        let route = remove_protected_mcp_route(&mut cfg, name)?;
        let restart_required =
            super::config_transaction::protected_routes_have_restart_debt(&runtime_cfg, &cfg);
        let result = staged_route_result(
            route.clone(),
            None,
            runtime_existing.as_ref(),
            restart_required,
        );
        let restart_required = result["restart_required"].as_bool().unwrap_or(true);
        if restart_required {
            self.persist_desired_config_owned(_mutation_guard, cfg)
                .await?;
        } else {
            // Once the last gateway-subset restart dependency is gone, every
            // remaining desired route difference is hot-safe. Publish that
            // accumulated direct-route state now so a no-restart result can
            // never get ahead of the live route index.
            self.persist_config_owned(_mutation_guard, cfg).await?;
        }
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.protected_route.stage_remove",
            route = %route.name,
            public_host = %route.public_host,
            public_path = %route.public_path,
            elapsed_ms = started.elapsed().as_millis(),
            restart_required,
            "protected MCP route desired removal saved"
        );
        Ok(result)
    }

    pub async fn protected_route_test(
        &self,
        route: ProtectedMcpRouteConfig,
    ) -> Result<Value, ToolError> {
        let mut cfg = self.config.read().await.clone();
        cfg.protected_mcp_routes.clear();
        let route = insert_protected_mcp_route(&mut cfg, route)?;
        crate::gateway::config::validate_config(&cfg)?;
        let resource = route.public_resource();
        let metadata_url = format!(
            "https://{}/.well-known/oauth-protected-resource{}",
            route.public_host, route.public_path
        );
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.protected_route.test",
            route = %route.name,
            resource = %resource,
            metadata_url = %metadata_url,
            upstream = ?route.upstream,
            backend_url = %route.backend_url,
            backend_mcp_path = %route.backend_mcp_path,
            scopes = ?route.scopes,
            "protected MCP route validated"
        );
        Ok(serde_json::json!({
            "ok": true,
            "route": route,
            "resource": resource,
            "metadata_url": metadata_url,
        }))
    }
}

fn staged_route_result(
    route: ProtectedMcpRouteConfig,
    desired: Option<&ProtectedMcpRouteConfig>,
    runtime: Option<&ProtectedMcpRouteConfig>,
    restart_required: bool,
) -> Value {
    let local_changed = desired != runtime;
    let pending_operation = if !local_changed {
        None
    } else if runtime.is_none() {
        Some("add")
    } else if desired.is_none() {
        Some("remove")
    } else {
        Some("update")
    };
    let restart_note = if restart_required && local_changed {
        "The protected route desired state was saved to durable config but this process is still serving the startup-mounted route set. Restart labby serve to apply it."
    } else if restart_required {
        "This route now matches its runtime state, but other protected route changes are still staged. Restart labby serve to apply the remaining desired route set."
    } else {
        "The desired protected route state now matches the route set mounted by this process; no restart is required."
    };
    json!({
        "route": route,
        "restart_required": restart_required,
        "pending_operation": pending_operation,
        "restart_note": restart_note,
    })
}

fn preserve_route_project_id(
    config: &GatewayConfig,
    name: &str,
    replacement: &mut ProtectedMcpRouteConfig,
) -> Result<(), ToolError> {
    let Some(existing) = config
        .protected_mcp_routes
        .iter()
        .find(|route| route.name == name)
    else {
        return Ok(());
    };
    match (&existing.target, &mut replacement.target) {
        (
            Some(ProtectedMcpRouteTarget::GatewaySubset(existing)),
            Some(ProtectedMcpRouteTarget::GatewaySubset(replacement)),
        ) => replacement.project_id.clone_from(&existing.project_id),
        _ => {
            return Err(ToolError::InvalidParam {
                message:
                    "project binding can only be preserved while updating a gateway_subset route"
                        .to_string(),
                param: "route.target".to_string(),
            });
        }
    }
    Ok(())
}

fn reject_pending_route_restart(
    runtime: &GatewayConfig,
    desired: &GatewayConfig,
    operation: &str,
) -> Result<(), ToolError> {
    if !super::config_transaction::protected_routes_have_restart_debt(runtime, desired) {
        return Ok(());
    }
    Err(ToolError::Sdk {
        sdk_kind: "restart_required".to_string(),
        message: format!(
            "protected MCP route changes are already staged for restart; restart labby serve before using hot {operation}, or continue editing the pending route through the staged actions"
        ),
    })
}

fn reject_hot_gateway_subset_mutation(
    route: &ProtectedMcpRouteConfig,
    operation: &str,
) -> Result<(), ToolError> {
    if !route.is_gateway_subset() {
        return Ok(());
    }
    Err(ToolError::Sdk {
        sdk_kind: "restart_required".to_string(),
        message: format!(
            "gateway_subset protected routes are mounted when labby serve starts; edit config and restart before `{operation}` can take effect"
        ),
    })
}
