//! Registry for first-class Labby operator apps.
//!
//! App metadata intentionally lives in the Labby product layer rather than in
//! `labby-primitives::ActionSpec`: actions describe callable behavior across
//! every surface, while apps describe UI shells, browser routes, and widget
//! bindings. The connection is explicit through `AppActionBinding` and resolved
//! against the live `ToolRegistry` so scopes/descriptions do not drift.

use serde::Serialize;

use crate::app_assets::{SERVER_LOGS_APP_URI, SERVER_LOGS_APP_URI_PREFIX};
use crate::registry::ToolRegistry;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AppKind {
    Browse,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct AppActionBinding {
    pub(crate) service: &'static str,
    pub(crate) action: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct AppDataRoute {
    pub(crate) label: &'static str,
    pub(crate) method: &'static str,
    pub(crate) path: &'static str,
    pub(crate) binding: AppActionBinding,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct AppSpec {
    pub(crate) slug: &'static str,
    pub(crate) title: &'static str,
    pub(crate) description: &'static str,
    pub(crate) icon: &'static str,
    pub(crate) kind: AppKind,
    pub(crate) browser_path: &'static str,
    pub(crate) ui_resource_uri: &'static str,
    pub(crate) ui_resource_prefix: &'static str,
    pub(crate) primary_action: AppActionBinding,
    pub(crate) data_routes: &'static [AppDataRoute],
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AppsManifest {
    pub(crate) kind: &'static str,
    pub(crate) apps: Vec<AppManifestEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AppManifestEntry {
    pub(crate) slug: &'static str,
    pub(crate) title: &'static str,
    pub(crate) description: &'static str,
    pub(crate) icon: &'static str,
    pub(crate) kind: AppKind,
    pub(crate) browser_path: &'static str,
    pub(crate) ui_resource_uri: &'static str,
    pub(crate) ui_resource_prefix: &'static str,
    pub(crate) required_scopes: Vec<&'static str>,
    pub(crate) primary_action: ResolvedActionBinding,
    pub(crate) data_routes: Vec<ResolvedDataRoute>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResolvedActionBinding {
    pub(crate) service: &'static str,
    pub(crate) action: &'static str,
    pub(crate) description: String,
    pub(crate) returns: String,
    pub(crate) requires_admin: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResolvedDataRoute {
    pub(crate) label: &'static str,
    pub(crate) method: &'static str,
    pub(crate) path: &'static str,
    pub(crate) binding: ResolvedActionBinding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppManifestError {
    pub(crate) app_slug: &'static str,
    pub(crate) service: &'static str,
    pub(crate) action: &'static str,
}

const SERVER_LOGS_DATA_ROUTES: &[AppDataRoute] = &[AppDataRoute {
    label: "query",
    method: "GET",
    path: "/v1/server-logs/query",
    binding: AppActionBinding {
        service: "server_logs",
        action: "server_logs.query",
    },
}];

const APPS: &[AppSpec] = &[AppSpec {
    slug: "server-logs",
    title: "Server Logs",
    description: "Browse and filter Labby's own rolling server process logs.",
    icon: "activity",
    kind: AppKind::Browse,
    browser_path: "/apps/server-logs",
    ui_resource_uri: SERVER_LOGS_APP_URI,
    ui_resource_prefix: SERVER_LOGS_APP_URI_PREFIX,
    primary_action: AppActionBinding {
        service: "server_logs",
        action: "server_logs.query",
    },
    data_routes: SERVER_LOGS_DATA_ROUTES,
}];

pub(crate) fn manifest_for_registry(
    registry: &ToolRegistry,
) -> Result<AppsManifest, AppManifestError> {
    Ok(AppsManifest {
        kind: "labby_apps",
        apps: APPS
            .iter()
            .map(|app| manifest_entry(registry, app))
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn manifest_entry(
    registry: &ToolRegistry,
    app: &AppSpec,
) -> Result<AppManifestEntry, AppManifestError> {
    let primary_action = resolve_binding(registry, app.slug, app.primary_action)?;
    let data_routes = app
        .data_routes
        .iter()
        .map(|route| {
            Ok(ResolvedDataRoute {
                label: route.label,
                method: route.method,
                path: route.path,
                binding: resolve_binding(registry, app.slug, route.binding)?,
            })
        })
        .collect::<Result<Vec<_>, AppManifestError>>()?;
    let required_scopes = if primary_action.requires_admin
        || data_routes.iter().any(|route| route.binding.requires_admin)
    {
        vec!["lab:admin"]
    } else {
        vec!["lab:read"]
    };
    Ok(AppManifestEntry {
        slug: app.slug,
        title: app.title,
        description: app.description,
        icon: app.icon,
        kind: app.kind,
        browser_path: app.browser_path,
        ui_resource_uri: app.ui_resource_uri,
        ui_resource_prefix: app.ui_resource_prefix,
        required_scopes,
        primary_action,
        data_routes,
    })
}

fn resolve_binding(
    registry: &ToolRegistry,
    app_slug: &'static str,
    binding: AppActionBinding,
) -> Result<ResolvedActionBinding, AppManifestError> {
    let Some(service) = registry.service(binding.service) else {
        return Err(AppManifestError {
            app_slug,
            service: binding.service,
            action: binding.action,
        });
    };
    let Some(action) = service
        .actions
        .iter()
        .find(|action| action.name == binding.action)
    else {
        return Err(AppManifestError {
            app_slug,
            service: binding.service,
            action: binding.action,
        });
    };
    Ok(ResolvedActionBinding {
        service: binding.service,
        action: binding.action,
        description: action.description.to_string(),
        returns: action.returns.to_string(),
        requires_admin: action.requires_admin,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::build_default_registry;

    #[test]
    fn app_manifest_derives_server_logs_scope_from_action_spec() {
        let registry = build_default_registry();
        let manifest = manifest_for_registry(&registry).expect("manifest");
        let app = manifest
            .apps
            .iter()
            .find(|app| app.slug == "server-logs")
            .expect("server logs app");

        assert_eq!(app.required_scopes, vec!["lab:admin"]);
        assert_eq!(app.primary_action.service, "server_logs");
        assert_eq!(app.primary_action.action, "server_logs.query");
        assert_eq!(app.ui_resource_prefix, SERVER_LOGS_APP_URI_PREFIX);
        assert!(app.ui_resource_uri.starts_with(app.ui_resource_prefix));
        assert!(app.primary_action.requires_admin);
        assert!(
            app.primary_action
                .description
                .contains("rolling JSON server process logs")
        );
    }

    #[test]
    fn app_manifest_validates_all_static_bindings() {
        let registry = build_default_registry();
        manifest_for_registry(&registry)
            .expect("app specs reference registered ActionSpec entries");
    }
}
