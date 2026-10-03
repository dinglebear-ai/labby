//! Service-free preparation for native Tailcat; owner authority is never fabricated.
use std::path::Path;

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::config::host_write::{HostConfigLock, read_config_snapshot};

/// Validate installed companions before prompting for OAuth credentials.
/// Reading and dry-run do not create a home, lock file, credential, or listener.
pub(crate) fn inspect(config_path: &Path) -> Result<Value> {
    let raw = read_config_snapshot(config_path)?;
    let config: crate::config::LabConfig = toml::from_str(&raw)?;
    let bundle = crate::dispatch::tailcat::assets::discover(config.tailcat.bundle_path.as_deref())?;
    if config.tailcat.enabled {
        bail!(
            "Tailcat is already enabled; use labby tailcat status to inspect the existing controller instead of preparing a new installation"
        );
    }
    Ok(json!({
        "helper": bundle.helper,
        "adapter": bundle.adapter,
        "browser_assets": bundle.browser,
        "authorization_required": true,
        "controller_enabled": false,
        "service_installed": false,
        "next_steps": [
            "Start the OAuth-protected native gateway with labby serve.",
            "Complete the existing authenticated owner bootstrap for a real project.",
            "Configure a tools-only adapter loadout and project-bound protected route, then enable Tailcat.",
            "Approve dashboard pairing locally with labby tailcat pair."
        ]
    }))
}

/// Persist only disabled transport preferences using the shared host lock.
/// OAuth setup owns its .env transaction; no access-store rows are inserted here.
pub(crate) fn prepare(config_path: &Path) -> Result<()> {
    let lock = HostConfigLock::acquire(config_path)?;
    let mut document = lock.read()?;
    let config: crate::config::LabConfig = toml::from_str(&document.to_string())?;
    if config.tailcat.enabled {
        bail!(
            "Tailcat configuration changed during setup; existing enabled controller was preserved"
        );
    }
    // Recheck under the owning configuration lock before committing.
    crate::dispatch::tailcat::assets::discover(config.tailcat.bundle_path.as_deref())?;
    if !document.contains_key("tailcat") {
        document["tailcat"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    document["tailcat"]["enabled"] = toml_edit::value(false);
    lock.write(&document.to_string())?;
    Ok(())
}

/// Detect the persisted inbound OAuth mode without applying ambient environment.
pub(crate) fn has_existing_oauth(root: &Path) -> Result<bool> {
    let raw = read_config_snapshot(&root.join(".env"))?;
    let mut mode = None;
    for entry in dotenvy::from_read_iter(raw.as_bytes()) {
        let (key, value) =
            entry.map_err(|_| anyhow::anyhow!("existing server environment is invalid"))?;
        if key == "LABBY_AUTH_MODE" {
            mode = Some(value);
        }
    }
    let config: crate::config::LabConfig =
        toml::from_str(&read_config_snapshot(&root.join("config.toml"))?)?;
    Ok(mode
        .or_else(|| config.auth.and_then(|auth| auth.mode))
        .as_deref()
        == Some("oauth"))
}

/// Input never carries identity, credentials, command text, or broadened exposure.
#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfigureRequest {
    pub project_id: String,
    pub public_resource: String,
    pub derp_map_url: String,
    pub node_path: std::path::PathBuf,
    #[serde(default)]
    pub dry_run: bool,
}

pub(super) fn denied() -> crate::dispatch::error::ToolError {
    crate::dispatch::error::ToolError::Forbidden {
        message: "Tailcat configuration requires the local operator and an active project owner"
            .into(),
        required_scopes: vec![],
    }
}
fn invalid(field: &str) -> crate::dispatch::error::ToolError {
    crate::dispatch::error::ToolError::InvalidParam {
        message: format!("Invalid Tailcat {field}"),
        param: field.into(),
    }
}
pub(super) fn unavailable() -> crate::dispatch::error::ToolError {
    crate::dispatch::error::ToolError::Sdk {
        sdk_kind: "tailcat_setup_unavailable".into(),
        message: "Tailcat setup prerequisites are unavailable; inspect local configuration and verified release companions".into(),
    }
}
pub(super) fn map_config_error(
    error: crate::config::host_write::HostWriteError,
) -> crate::dispatch::error::ToolError {
    use crate::config::host_write::HostWriteError;
    crate::dispatch::error::ToolError::Sdk {
        sdk_kind: match error {
            HostWriteError::Durability => "durability_uncertain",
            HostWriteError::Busy => "configuration_busy",
            _ => "configuration_io_error",
        }
        .into(),
        message: error.to_string(),
    }
}
fn now_millis() -> std::result::Result<u64, crate::dispatch::error::ToolError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|value| u64::try_from(value.as_millis()).ok())
        .ok_or_else(unavailable)
}

/// Live completion uses only the authenticated identity supplied by native middleware.
/// Both host operator consent and durable project ownership are required.
pub(crate) async fn configure(
    caller: super::SetupCaller,
    store: crate::access::AccessStore,
    identity: labby_auth::VerifiedIdentity,
    ceiling: crate::access::AuthorityCeiling,
    config_path: std::path::PathBuf,
    request: ConfigureRequest,
) -> std::result::Result<Value, crate::dispatch::error::ToolError> {
    use crate::access::{ActionAuthoritySpec, AuthorityRequest, ProjectRole};
    use labby_primitives::access::{
        ActionRef, Capability, OwnerScope, ProjectId, ResourceFamily, ResourceId, ResourceRef,
    };
    use labby_runtime::authority::AuthoritySafeBoundary;
    if caller != super::SetupCaller::Operator
        || identity.authenticator() != labby_auth::Authenticator::BrowserSession
    {
        return Err(denied());
    }
    let owner = OwnerScope::Project(
        ProjectId::new(request.project_id.clone()).map_err(|_| invalid("project_id"))?,
    );
    let projects = store
        .list_effective_projects(identity.clone())
        .await
        .map_err(|_| denied())?;
    if !projects.iter().any(|project| {
        project.project_id == request.project_id && project.role == ProjectRole::Owner
    }) {
        return Err(denied());
    }
    let action = ActionRef::new("setup", "tailcat.configure").map_err(|_| unavailable())?;
    let lease = crate::access::authorize_action(
        &store,
        AuthorityRequest::new(
            identity.clone(),
            ActionAuthoritySpec::SCHEMA_VERSION,
            action.clone(),
            ResourceRef::new(
                owner.clone(),
                ResourceFamily::Project,
                ResourceId::new(request.project_id.clone()).map_err(|_| invalid("project_id"))?,
            ),
            ceiling,
            None,
            now_millis()?,
            vec![AuthoritySafeBoundary::BeforeCommit],
            vec![ActionAuthoritySpec::new(
                action,
                ResourceFamily::Project,
                Capability::ScopeManage,
            )],
        ),
    )
    .await
    .map_err(|_| denied())?;
    // Resolve filesystem prerequisites and acquire the owning host lock outside async workers.
    let dry_run = request.dry_run;
    let project_id = request.project_id.clone();
    let prepared = tokio::task::spawn_blocking(move || prepare_live_config(&config_path, &request))
        .await
        .map_err(|_| unavailable())??;
    let (lock, rendered, outcome) = prepared;
    store
        .commit_tailcat_configuration(identity, project_id, lease, move || {
            if dry_run {
                return Ok(());
            }
            lock.ok_or_else(unavailable)?
                .write(&rendered)
                .map_err(map_config_error)
        })
        .await
        .map_err(|error| {
            crate::dispatch::access_errors::map_store_error("setup", error, denied)
        })??;
    Ok(outcome)
}

type PreparedConfiguration = (Option<HostConfigLock>, String, Value);
fn prepare_live_config(
    path: &Path,
    request: &ConfigureRequest,
) -> std::result::Result<PreparedConfiguration, crate::dispatch::error::ToolError> {
    // Read-only previews deliberately acquire no lock file or installation state.
    let lock = if request.dry_run {
        None
    } else {
        Some(HostConfigLock::acquire(path).map_err(|_| unavailable())?)
    };
    let raw = match &lock {
        Some(lock) => lock.read_raw(),
        None => read_config_snapshot(path),
    }
    .map_err(|_| unavailable())?;
    let mut config: crate::config::LabConfig = toml::from_str(&raw).map_err(|_| unavailable())?;
    if config.tailcat.enabled {
        return Err(invalid("existing_enabled_controller"));
    }
    let bundle = crate::dispatch::tailcat::assets::discover(config.tailcat.bundle_path.as_deref())
        .map_err(|_| unavailable())?;
    let (rendered, outcome) = render_live_config(&raw, &mut config, request, &bundle.adapter)?;
    Ok((lock, rendered, outcome))
}

pub(super) fn render_live_config(
    raw: &str,
    config: &mut crate::config::LabConfig,
    request: &ConfigureRequest,
    adapter: &Path,
) -> std::result::Result<(String, Value), crate::dispatch::error::ToolError> {
    use sha2::Digest as _;
    if request.public_resource.len() > 2048 || request.derp_map_url.len() > 2048 {
        return Err(invalid("url_length"));
    }
    let resource =
        url::Url::parse(&request.public_resource).map_err(|_| invalid("public_resource"))?;
    if resource.scheme() != "https"
        || !resource.username().is_empty()
        || resource.password().is_some()
        || resource.query().is_some()
        || resource.fragment().is_some()
        || resource.host_str().is_none()
        || resource.port().is_some()
        || resource.path() == "/"
    {
        return Err(invalid("public_resource"));
    }
    let derp = url::Url::parse(&request.derp_map_url).map_err(|_| invalid("derp_map_url"))?;
    if derp.scheme() != "https"
        || !derp.username().is_empty()
        || derp.password().is_some()
        || derp.fragment().is_some()
    {
        return Err(invalid("derp_map_url"));
    }
    if !request.node_path.is_absolute()
        || request
            .node_path
            .file_name()
            .is_none_or(|name| name != "node")
    {
        return Err(invalid("node_path"));
    }
    let node = request
        .node_path
        .canonicalize()
        .map_err(|_| invalid("node_path"))?;
    use std::os::unix::fs::PermissionsExt as _;
    if !node.is_file()
        || node
            .metadata()
            .map_err(|_| invalid("node_path"))?
            .permissions()
            .mode()
            & 0o111
            == 0
    {
        return Err(invalid("node_path"));
    }
    let suffix = hex::encode(sha2::Sha256::digest(request.project_id.as_bytes()));
    let name = format!("tailcat-{}", &suffix[..16]);
    let upstream: crate::config::UpstreamConfig = serde_json::from_value(json!({
        "name":name, "command":node, "args":[adapter],
        "env":{"MICROSANDBOX_MCP_HOST_PATH_POLICY":"allowlist", "MICROSANDBOX_MCP_HOST_PATHS":":", "MICROSANDBOX_MCP_ENABLE_DANGEROUS":"0", "MSB_BACKEND":"local"},
        "proxy_resources":false,"proxy_prompts":false,
        "expose_tools":["sandbox_create","sandbox_exec","sandbox_stop","sandbox_remove","sandbox_list","sandbox_inspect"]
    })).map_err(|_| unavailable())?;
    let loadout = crate::config::GatewayLoadoutConfig {
        name: name.clone(),
        upstreams: vec![name.clone()],
        expose_resources: false,
        expose_prompts: false,
        expose_skills: false,
        ..Default::default()
    };
    let route: crate::config::ProtectedMcpRouteConfig = serde_json::from_value(json!({
        "name":name, "public_host":resource.host_str(), "public_path":resource.path(), "scopes":["lab"],
        "target":{"kind":"gateway_subset","project_id":request.project_id,"loadout":name}
    })).map_err(|_| unavailable())?;
    if config
        .protected_mcp_routes
        .iter()
        .any(|old| old.name != name && old.public_resource() == route.public_resource())
    {
        return Err(invalid("route_conflict"));
    }
    insert_exact(&mut config.upstream, &name, upstream, |entry| &entry.name)?;
    insert_exact(&mut config.loadouts, &name, loadout, |entry| &entry.name)?;
    insert_exact(&mut config.protected_mcp_routes, &name, route, |entry| {
        &entry.name
    })?;
    if config
        .tailcat
        .derp_map_url
        .as_ref()
        .is_some_and(|existing| existing != &request.derp_map_url)
    {
        return Err(invalid("derp_map_conflict"));
    }
    config.tailcat.derp_map_url = Some(request.derp_map_url.clone());
    config.validate().map_err(|_| invalid("configuration"))?;
    let mut document: toml_edit::DocumentMut = raw.parse().map_err(|_| unavailable())?;
    let desired: toml_edit::DocumentMut = toml::to_string(config)
        .map_err(|_| unavailable())?
        .parse()
        .map_err(|_| unavailable())?;
    for key in ["upstream", "loadouts", "protected_mcp_routes", "tailcat"] {
        document[key] = desired[key].clone();
    }
    let rendered = document.to_string();
    let outcome = json!({"configured":true,"changed":rendered != raw,"dry_run":request.dry_run,"project_id":request.project_id,
        "upstream":name,"loadout":name,"route":name,"controller_enabled":false,"restart_required":true,
        "credential_enrollment_required":true,"service_installed":false});
    Ok((rendered, outcome))
}

fn insert_exact<T: serde::Serialize>(
    entries: &mut Vec<T>,
    name: &str,
    desired: T,
    key: impl Fn(&T) -> &str,
) -> std::result::Result<(), crate::dispatch::error::ToolError> {
    if let Some(existing) = entries.iter().find(|entry| key(entry) == name) {
        if serde_json::to_value(existing).ok() != serde_json::to_value(&desired).ok() {
            return Err(invalid("existing_configuration_conflict"));
        }
    } else {
        entries.push(desired);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    #[tokio::test]
    async fn live_setup_rejects_delegation_and_another_project_before_config_io() {
        let (directory, runtime, _adapter, _approved) =
            crate::dispatch::tailcat::testing::fixture().await;
        let store = runtime.store().await.unwrap();
        let identity = labby_auth::VerifiedIdentity::external(
            labby_auth::Authenticator::BrowserSession,
            "https://accounts.google.com",
            "test-operator",
        )
        .unwrap();
        for (caller, project) in [
            (super::super::SetupCaller::Delegated, "bootstrap-default"),
            (super::super::SetupCaller::Operator, "another-project"),
        ] {
            let (_, mut request) = render_fixture();
            request.project_id = project.into();
            let config = directory.path().join("uncreated/config.toml");
            let auth = labby_auth::auth_context::AuthContext {
                sub: "test-operator".into(),
                actor_key: None,
                scopes: vec!["lab".into()],
                issuer: "browser-session".into(),
                via_session: true,
                csrf_token: None,
                email: None,
            };
            let error = super::configure(
                caller,
                store.clone(),
                identity.clone(),
                crate::access::AuthorityCeiling::from_auth_context(&auth),
                config.clone(),
                request,
            )
            .await
            .unwrap_err();
            assert!(matches!(
                error,
                crate::dispatch::error::ToolError::Forbidden { .. }
            ));
            assert!(!config.parent().unwrap().exists());
        }
    }

    fn render_fixture() -> (tempfile::TempDir, super::ConfigureRequest) {
        use std::os::unix::fs::PermissionsExt as _;
        let temp = tempfile::tempdir().unwrap();
        let node = temp.path().join("node");
        std::fs::write(&node, "fixture").unwrap();
        std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o700)).unwrap();
        let request = super::ConfigureRequest {
            project_id: "project-one".into(),
            public_resource: "https://labby.example/sandbox".into(),
            derp_map_url: "https://tailcat.example/derpmap.json".into(),
            node_path: node,
            dry_run: true,
        };
        (temp, request)
    }

    #[test]
    fn restricted_plan_is_idempotent_and_preserves_foreign_keys() {
        let (_temp, request) = render_fixture();
        let raw = "[foreign]\nkeep = 'yes'\n";
        let mut config = toml::from_str(raw).unwrap();
        let (rendered, outcome) = super::render_live_config(
            raw,
            &mut config,
            &request,
            Path::new("/verified/adapter/server.mjs"),
        )
        .unwrap();
        assert!(rendered.contains("keep = \"yes\"") || rendered.contains("keep = 'yes'"));
        assert_eq!(outcome["controller_enabled"], false);
        assert_eq!(outcome["credential_enrollment_required"], true);
        assert_eq!(config.loadouts.len(), 1);
        let loadout = &config.loadouts[0];
        assert!(
            !loadout.expose_resources
                && !loadout.expose_prompts
                && !loadout.expose_skills
                && !loadout.expose_code_mode
        );
        assert!(loadout.services.is_empty());
        assert_eq!(config.upstream[0].env["MICROSANDBOX_MCP_HOST_PATHS"], ":");
        let (again, outcome) = super::render_live_config(
            &rendered,
            &mut config,
            &request,
            Path::new("/verified/adapter/server.mjs"),
        )
        .unwrap();
        assert_eq!(rendered, again);
        assert_eq!(outcome["changed"], false);
    }

    #[test]
    fn existing_policy_conflicts_are_not_overwritten() {
        let (_temp, request) = render_fixture();
        let mut config = crate::config::LabConfig::default();
        super::render_live_config(
            "",
            &mut config,
            &request,
            Path::new("/verified/adapter/server.mjs"),
        )
        .unwrap();
        config.loadouts[0].expose_code_mode = true;
        assert!(
            super::render_live_config(
                "",
                &mut config,
                &request,
                Path::new("/verified/adapter/server.mjs")
            )
            .is_err()
        );
        assert!(config.loadouts[0].expose_code_mode);
    }

    #[test]
    fn oauth_mode_precedence_is_persisted_and_no_write_occurs() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("config.toml"),
            "[auth]\nmode = 'oauth'\n[foreign]\nkeep = true\n",
        )
        .unwrap();
        assert!(super::has_existing_oauth(temp.path()).unwrap());
        std::fs::write(temp.path().join(".env"), "LABBY_AUTH_MODE=bearer\n").unwrap();
        assert!(!super::has_existing_oauth(temp.path()).unwrap());
        assert!(!temp.path().join("config.toml.lock").exists());
        assert!(
            std::fs::read_to_string(temp.path().join("config.toml"))
                .unwrap()
                .contains("keep = true")
        );
    }

    #[test]
    fn malformed_existing_env_fails_without_echoing_values() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join(".env"), "LABBY_AUTH_MODE='secret-unclosed").unwrap();
        let error = super::has_existing_oauth(temp.path())
            .unwrap_err()
            .to_string();
        assert!(!error.contains("secret-unclosed"));
    }

    #[test]
    fn missing_bundle_inspection_does_not_create_configuration() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("new-home/config.toml");
        assert!(super::inspect(&config).is_err());
        assert!(!config.parent().unwrap().exists());
    }
}
