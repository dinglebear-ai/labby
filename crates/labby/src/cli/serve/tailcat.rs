//! Compose the opt-in private controller from the hosted gateway's authority.
use crate::{
    api::{AppState, tailcat::ControlListener},
    config::McpPreferences,
    dispatch::tailcat::{
        HelperArtifact,
        manager::{Manager, NativeOwner},
    },
};
use anyhow::{Context as _, Result};
use std::sync::Arc;

pub(super) async fn start_control(
    state: &AppState,
    auth: Option<&labby_auth::state::AuthState>,
    mcp: &McpPreferences,
    notifier: crate::mcp::peers::PeerNotifier,
) -> Result<Option<ControlListener>> {
    let preferences = &state.config.tailcat;
    if !preferences.enabled {
        return Ok(None);
    }
    labby_tailcat::ensure_platform_supported().context("Tailcat platform unsupported")?;
    let auth = auth
        .filter(|auth| auth.config.mode == labby_auth::config::AuthMode::OAuth)
        .filter(|_| !state.web_ui_auth_disabled)
        .ok_or_else(|| anyhow::anyhow!("Tailcat requires enabled OAuth browser authentication"))?;
    let key =
        auth.config.token_encryption_key.clone().ok_or_else(|| {
            anyhow::anyhow!("Tailcat requires the persistent OAuth encryption key")
        })?;
    let adapter = state
        .access_credential_adapter
        .clone()
        .ok_or_else(|| anyhow::anyhow!("Tailcat requires native project credential authority"))?;
    let gateway = state
        .gateway_manager
        .clone()
        .ok_or_else(|| anyhow::anyhow!("Tailcat requires the hosted gateway"))?;
    let installation = state
        .installation_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Tailcat requires an initialized installation"))?
        .to_owned();
    let executable = preferences.helper_path.clone().ok_or_else(|| {
        anyhow::anyhow!("configure tailcat.helper_path with the pinned native helper")
    })?;
    let checksum = preferences.helper_sha256.as_ref().ok_or_else(|| {
        anyhow::anyhow!("configure tailcat.helper_sha256 from the pinned asset manifest")
    })?;
    let expected_sha256: [u8; 32] = hex::decode(checksum)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| anyhow::anyhow!("tailcat.helper_sha256 must be a SHA-256 digest"))?;
    let derp_map_url = preferences
        .derp_map_url
        .clone()
        .ok_or_else(|| anyhow::anyhow!("configure an approved HTTPS tailcat.derp_map_url"))?;
    let state_dir = labby_runtime::lab_home().join("tailcat");
    crate::dispatch::setup::secure_file::create_private_dir(&state_dir)
        .context("initialize private Tailcat helper state directory")?;
    // Verify artifact and map before publishing even the private control socket.
    // Actual startup validates another immutable snapshot against this digest.
    let preflight = labby_tailcat::BridgeConfig {
        executable: executable.clone(),
        state_dir: state_dir.clone(),
        expected_sha256,
        target: "127.0.0.1:1".parse()?,
        peer: format!("nodekey:{}", "1".repeat(64)),
        derp_map_url: derp_map_url.clone(),
    };
    tokio::task::spawn_blocking(move || preflight.validate())
        .await?
        .context("Tailcat pinned helper validation failed")?;
    let mut projection_state = state.clone().with_oauth_state(auth.clone());
    if let Some(routers) = super::build_protected_mcp_routers(&projection_state, mcp, notifier)? {
        projection_state = projection_state.with_protected_mcp_routers(routers);
    }
    let artifact = HelperArtifact {
        executable,
        state_dir,
        expected_sha256,
        derp_map_url,
    };
    let owner = NativeOwner {
        oauth_enabled: true,
        runtime: state.access_runtime.clone(),
        adapter,
        gateway,
        installation,
        key: Arc::new(key),
        projection: Arc::new(move |authority, route| {
            crate::api::tailcat::restricted_router(projection_state.clone(), route, authority)
        }),
    };
    let manager = Arc::new(Manager::new(owner, artifact)?);
    ControlListener::start(preferences.socket_path(), manager)
        .await
        .context("publish private Tailcat control socket")
        .map(Some)
}
