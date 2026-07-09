//! HTTP bridge to a `labby serve` instance's catalog + action-dispatch API.
//!
//! Two calls: `GET /v1/catalog` (service/action discovery) and
//! `POST /v1/{service}` (`{action, params}` dispatch). Auth is resolved by
//! `oauth::send_with_reauth`, which prefers a live OAuth access token and falls
//! back to the static bearer token from settings.

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::{merged_settings, validate_saved_server_url};

const BRIDGE_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
const BRIDGE_TOTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// A shared `reqwest::Client` held in Tauri `AppState` so every bridge call
/// reuses one connection pool / TLS context.
pub(crate) struct BridgeClient(reqwest::Client);

impl BridgeClient {
    pub(crate) fn new() -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .timeout(BRIDGE_TOTAL_TIMEOUT)
            .connect_timeout(BRIDGE_CONNECT_TIMEOUT)
            .user_agent(concat!("Labby Palette/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self(client))
    }

    pub(crate) fn client(&self) -> &reqwest::Client {
        &self.0
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LabbyHttpResult {
    ok: bool,
    status: u16,
    payload: serde_json::Value,
}

/// Only a plain service identifier — no path separators, no scheme — so the
/// dispatch path can never escape `/v1/{service}`.
fn validate_service_name(service: &str) -> Result<(), String> {
    let valid = !service.is_empty()
        && service
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-');
    valid
        .then_some(())
        .ok_or_else(|| "service name must be alphanumeric (with `_`/`-`)".to_string())
}

#[tauri::command]
pub(crate) async fn fetch_catalog(
    app: AppHandle,
    bridge: tauri::State<'_, BridgeClient>,
    oauth_state: tauri::State<'_, crate::oauth::OauthState>,
    etag: Option<String>,
) -> Result<LabbyHttpResult, String> {
    let settings = merged_settings(&app)?;
    let base_url = validate_saved_server_url(&settings.server_url)?;
    let url = format!("{}/v1/catalog", base_url.trim_end_matches('/'));
    let client = (*bridge).client();
    let static_token = settings
        .static_token
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());

    let make = |token: Option<&str>| {
        let mut b = client
            .get(&url)
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(t) = token {
            b = b.bearer_auth(t);
        }
        if let Some(etag) = &etag {
            b = b.header(reqwest::header::IF_NONE_MATCH, etag);
        }
        b
    };
    let response =
        crate::oauth::send_with_reauth(&app, client, &base_url, static_token, &oauth_state, make)
            .await?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(LabbyHttpResult {
            ok: true,
            status: status.as_u16(),
            payload: serde_json::Value::Null,
        });
    }
    let text = response.text().await.map_err(|err| err.to_string())?;
    let payload = if text.trim().is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text))
    };
    Ok(LabbyHttpResult {
        ok: status.is_success(),
        status: status.as_u16(),
        payload,
    })
}

#[derive(Debug, Deserialize)]
pub(crate) struct DispatchRequest {
    service: String,
    action: String,
    params: serde_json::Value,
}

#[tauri::command]
pub(crate) async fn dispatch_action(
    app: AppHandle,
    bridge: tauri::State<'_, BridgeClient>,
    oauth_state: tauri::State<'_, crate::oauth::OauthState>,
    request: DispatchRequest,
) -> Result<LabbyHttpResult, String> {
    validate_service_name(&request.service)?;
    let settings = merged_settings(&app)?;
    let base_url = validate_saved_server_url(&settings.server_url)?;
    let url = format!("{}/v1/{}", base_url.trim_end_matches('/'), request.service);
    let client = (*bridge).client();
    let static_token = settings
        .static_token
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let body = serde_json::json!({ "action": request.action, "params": request.params });

    let make = |token: Option<&str>| {
        let mut b = client
            .post(&url)
            .header(reqwest::header::ACCEPT, "application/json")
            .json(&body);
        if let Some(t) = token {
            b = b.bearer_auth(t);
        }
        b
    };
    let response =
        crate::oauth::send_with_reauth(&app, client, &base_url, static_token, &oauth_state, make)
            .await?;
    let status = response.status();
    let text = response.text().await.map_err(|err| err.to_string())?;
    let payload = if text.trim().is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text))
    };
    Ok(LabbyHttpResult {
        ok: status.is_success(),
        status: status.as_u16(),
        payload,
    })
}
