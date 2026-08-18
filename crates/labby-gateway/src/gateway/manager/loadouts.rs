//! Manager-level Loadout CRUD for reusable gateway capability projections.

use crate::gateway::config::{insert_loadout, remove_loadout, update_loadout};
use crate::gateway::params::GatewayLoadoutPatch;
use labby_runtime::error::ToolError;
use labby_runtime::gateway_config::{GatewayConfig, GatewayLoadoutConfig, ProtectedMcpRouteTarget};
use serde_json::{Value, json};
use std::collections::BTreeSet;

use super::GatewayManager;

impl GatewayManager {
    pub async fn loadout_list(&self) -> Vec<GatewayLoadoutConfig> {
        let mut loadouts = self.config.read().await.loadouts.clone();
        loadouts.sort_by(|left, right| left.name.cmp(&right.name));
        loadouts
    }

    pub async fn loadout_list_state(&self) -> Result<Vec<Value>, ToolError> {
        let desired_cfg = self.load_config_for_mutation().await?;
        let runtime_cfg = self.config.read().await.clone();
        let names = desired_cfg
            .loadouts
            .iter()
            .chain(runtime_cfg.loadouts.iter())
            .map(|loadout| loadout.name.clone())
            .collect::<BTreeSet<_>>();
        let mut rows = Vec::with_capacity(names.len());
        for name in names {
            let desired = desired_cfg
                .loadouts
                .iter()
                .find(|loadout| loadout.name == name);
            let runtime = runtime_cfg
                .loadouts
                .iter()
                .find(|loadout| loadout.name == name);
            let changed = desired != runtime;
            let route_related = loadout_has_enabled_route(&desired_cfg, &name)
                || loadout_has_enabled_route(&runtime_cfg, &name);
            let restart_required = changed && route_related;
            let pending_operation = if !restart_required {
                None
            } else if runtime.is_none() {
                Some("add")
            } else if desired.is_none() {
                Some("remove")
            } else {
                Some("update")
            };
            let display = desired.or(runtime).expect("name came from one loadout set");
            let mut value = serde_json::to_value(display).map_err(|error| {
                ToolError::internal_message(format!("failed to serialize loadout state: {error}"))
            })?;
            let object = value.as_object_mut().ok_or_else(|| {
                ToolError::internal_message("loadout state did not serialize as an object")
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
                Value::Bool(runtime.is_some()),
            );
            object.insert(
                "desired_present".to_string(),
                Value::Bool(desired.is_some()),
            );
            rows.push(value);
        }
        Ok(rows)
    }

    pub async fn loadout_get(&self, name: &str) -> Result<GatewayLoadoutConfig, ToolError> {
        self.load_config_for_mutation()
            .await?
            .loadouts
            .into_iter()
            .find(|loadout| loadout.name == name)
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!(
                    "loadout `{name}` not found in desired config; run `gateway.loadout.list_state` or `labby gateway loadout list` to inspect pending restart state"
                ),
            })
    }

    pub async fn loadout_add(
        &self,
        loadout: GatewayLoadoutConfig,
    ) -> Result<GatewayLoadoutConfig, ToolError> {
        self.validate_loadout_services(&loadout)?;
        let started = std::time::Instant::now();
        let _mutation_guard = self.acquire_config_mutation().await?;
        let mut cfg = self.load_config_for_mutation().await?;
        let loadout = insert_loadout(&mut cfg, loadout)?;
        self.persist_config_owned(_mutation_guard, cfg).await?;
        log_loadout_mutation("gateway.loadout.add", &loadout, started.elapsed());
        Ok(loadout)
    }

    pub async fn loadout_update(
        &self,
        name: &str,
        loadout: GatewayLoadoutConfig,
    ) -> Result<GatewayLoadoutConfig, ToolError> {
        self.validate_loadout_services(&loadout)?;
        let started = std::time::Instant::now();
        let _mutation_guard = self.acquire_config_mutation().await?;
        let mut cfg = self.load_config_for_mutation().await?;
        let runtime_cfg = self.config.read().await.clone();
        reject_hot_loadout_mutation(&cfg, &runtime_cfg, name, "update")?;
        let loadout = update_loadout(&mut cfg, name, loadout)?;
        self.persist_config_owned(_mutation_guard, cfg).await?;
        log_loadout_mutation("gateway.loadout.update", &loadout, started.elapsed());
        Ok(loadout)
    }

    pub(crate) async fn loadout_patch(
        &self,
        name: &str,
        patch: GatewayLoadoutPatch,
    ) -> Result<GatewayLoadoutConfig, ToolError> {
        let next = apply_loadout_patch(self.loadout_get(name).await?, patch);
        self.loadout_update(name, next).await
    }

    pub async fn loadout_stage_update(
        &self,
        name: &str,
        loadout: GatewayLoadoutConfig,
    ) -> Result<Value, ToolError> {
        self.validate_loadout_services(&loadout)?;
        let started = std::time::Instant::now();
        let _mutation_guard = self.acquire_config_mutation().await?;
        let mut cfg = self.load_config_for_mutation().await?;
        let runtime_cfg = self.config.read().await.clone();
        if !(loadout_has_enabled_route(&cfg, name) || loadout_has_enabled_route(&runtime_cfg, name))
        {
            return Err(ToolError::InvalidParam {
                message: "staging is only needed for a Loadout referenced by an enabled protected gateway route; use gateway.loadout.update for a hot-safe Loadout".to_string(),
                param: "name".to_string(),
            });
        }
        let loadout = update_loadout(&mut cfg, name, loadout)?;
        let runtime_loadout = runtime_cfg
            .loadouts
            .iter()
            .find(|runtime| runtime.name == loadout.name);
        let result = staged_loadout_result(loadout.clone(), Some(&loadout), runtime_loadout);
        self.persist_desired_config_owned(_mutation_guard, cfg)
            .await?;
        log_loadout_mutation("gateway.loadout.stage_update", &loadout, started.elapsed());
        Ok(result)
    }

    pub(crate) async fn loadout_stage_patch(
        &self,
        name: &str,
        patch: GatewayLoadoutPatch,
    ) -> Result<Value, ToolError> {
        let next = apply_loadout_patch(self.loadout_get(name).await?, patch);
        self.loadout_stage_update(name, next).await
    }

    pub async fn loadout_stage_remove(&self, name: &str) -> Result<Value, ToolError> {
        let started = std::time::Instant::now();
        let _mutation_guard = self.acquire_config_mutation().await?;
        let mut cfg = self.load_config_for_mutation().await?;
        let runtime_cfg = self.config.read().await.clone();
        if !(loadout_has_enabled_route(&cfg, name) || loadout_has_enabled_route(&runtime_cfg, name))
        {
            return Err(ToolError::InvalidParam {
                message: "staging is only needed for a Loadout still referenced by the running protected-route set; use gateway.loadout.remove for an unmounted Loadout".to_string(),
                param: "name".to_string(),
            });
        }
        // Desired config must already stop referencing this Loadout. This keeps
        // the next boot valid while allowing a route removal + Loadout removal
        // to be staged together without breaking the still-running route.
        let loadout = remove_loadout(&mut cfg, name)?;
        let runtime_loadout = runtime_cfg
            .loadouts
            .iter()
            .find(|runtime| runtime.name == loadout.name);
        let result = staged_loadout_result(loadout.clone(), None, runtime_loadout);
        self.persist_desired_config_owned(_mutation_guard, cfg)
            .await?;
        log_loadout_mutation("gateway.loadout.stage_remove", &loadout, started.elapsed());
        Ok(result)
    }

    pub async fn loadout_remove(&self, name: &str) -> Result<GatewayLoadoutConfig, ToolError> {
        let started = std::time::Instant::now();
        let _mutation_guard = self.acquire_config_mutation().await?;
        let mut cfg = self.load_config_for_mutation().await?;
        let runtime_cfg = self.config.read().await.clone();
        reject_hot_loadout_mutation(&cfg, &runtime_cfg, name, "remove")?;
        let loadout = remove_loadout(&mut cfg, name)?;
        self.persist_config_owned(_mutation_guard, cfg).await?;
        log_loadout_mutation("gateway.loadout.remove", &loadout, started.elapsed());
        Ok(loadout)
    }

    fn validate_loadout_services(&self, loadout: &GatewayLoadoutConfig) -> Result<(), ToolError> {
        for service in &loadout.services {
            if self.registered_service_meta(service).is_none() {
                return Err(ToolError::InvalidParam {
                    message: format!(
                        "loadout `{}` references unknown Lab service `{service}`; run `gateway.supported_services` to discover valid service names",
                        loadout.name
                    ),
                    param: "services".to_string(),
                });
            }
        }
        Ok(())
    }
}

fn apply_loadout_patch(
    mut loadout: GatewayLoadoutConfig,
    patch: GatewayLoadoutPatch,
) -> GatewayLoadoutConfig {
    if let Some(new_name) = patch.name {
        loadout.name = new_name;
    }
    if let Some(description) = patch.description {
        loadout.description = description;
    }
    if let Some(upstreams) = patch.upstreams {
        loadout.upstreams = upstreams;
    }
    if let Some(services) = patch.services {
        loadout.services = services;
    }
    if let Some(value) = patch.expose_code_mode {
        loadout.expose_code_mode = value;
    }
    if let Some(value) = patch.expose_tools {
        loadout.expose_tools = value;
    }
    if let Some(value) = patch.expose_resources {
        loadout.expose_resources = value;
    }
    if let Some(value) = patch.expose_prompts {
        loadout.expose_prompts = value;
    }
    if let Some(value) = patch.expose_skills {
        loadout.expose_skills = value;
    }
    loadout
}

fn loadout_has_enabled_route(cfg: &GatewayConfig, loadout: &str) -> bool {
    cfg.protected_mcp_routes.iter().any(|route| {
        if !route.enabled {
            return false;
        }
        let Some(ProtectedMcpRouteTarget::GatewaySubset(target)) = route.target.as_ref() else {
            return false;
        };
        target.loadout.as_deref() == Some(loadout)
    })
}

fn active_loadout_routes<'a>(cfg: &'a GatewayConfig, loadout: &str) -> Vec<&'a str> {
    cfg.protected_mcp_routes
        .iter()
        .filter(|route| route.enabled)
        .filter_map(|route| {
            let ProtectedMcpRouteTarget::GatewaySubset(target) = route.target.as_ref()?;
            (target.loadout.as_deref() == Some(loadout)).then_some(route.name.as_str())
        })
        .collect()
}

fn staged_loadout_result(
    loadout: GatewayLoadoutConfig,
    desired: Option<&GatewayLoadoutConfig>,
    runtime: Option<&GatewayLoadoutConfig>,
) -> Value {
    let restart_required = desired != runtime;
    let pending_operation = if !restart_required {
        None
    } else if runtime.is_none() {
        Some("add")
    } else if desired.is_none() {
        Some("remove")
    } else {
        Some("update")
    };
    let restart_note = if restart_required {
        "The Loadout desired state was saved to durable config but a protected gateway route in this process is still mounted with the startup Loadout projection. Restart labby serve to apply it."
    } else {
        "The desired Loadout state now matches the projection mounted by this process; no restart is required."
    };
    json!({
        "loadout": loadout,
        "restart_required": restart_required,
        "pending_operation": pending_operation,
        "restart_note": restart_note,
    })
}

fn reject_hot_loadout_mutation(
    desired_cfg: &GatewayConfig,
    runtime_cfg: &GatewayConfig,
    loadout: &str,
    operation: &str,
) -> Result<(), ToolError> {
    let mut active_routes = active_loadout_routes(desired_cfg, loadout);
    active_routes.extend(active_loadout_routes(runtime_cfg, loadout));
    active_routes.sort_unstable();
    active_routes.dedup();
    if active_routes.is_empty() {
        return Ok(());
    }
    Err(ToolError::Sdk {
        sdk_kind: "restart_required".to_string(),
        message: format!(
            "loadout `{loadout}` is mounted by enabled protected MCP route(s) {}; disable or edit those routes and restart `labby serve` before `{operation}` can take effect",
            active_routes.join(", ")
        ),
    })
}

fn log_loadout_mutation(
    action: &'static str,
    loadout: &GatewayLoadoutConfig,
    elapsed: std::time::Duration,
) {
    tracing::info!(
        surface = "dispatch",
        service = "gateway",
        action,
        loadout = %loadout.name,
        upstreams = loadout.upstreams.len(),
        services = loadout.services.len(),
        expose_tools = loadout.expose_tools,
        expose_resources = loadout.expose_resources,
        expose_prompts = loadout.expose_prompts,
        expose_skills = loadout.expose_skills,
        expose_code_mode = loadout.expose_code_mode,
        elapsed_ms = elapsed.as_millis(),
        "gateway loadout mutation complete"
    );
}
