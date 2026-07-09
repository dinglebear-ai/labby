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
const WRONG_API_HOST_HINT: &str = "Labby API returned HTML; configure LABBY_API_URL or the palette server URL to the Labby API origin, not the web UI origin";

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
    let content_type = response_content_type(&response);
    let text = response.text().await.map_err(|err| err.to_string())?;
    let payload = parse_json_payload(content_type.as_deref(), &text)?;
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
    let content_type = response_content_type(&response);
    let text = response.text().await.map_err(|err| err.to_string())?;
    let payload = parse_json_payload(content_type.as_deref(), &text)?;
    Ok(LabbyHttpResult {
        ok: status.is_success(),
        status: status.as_u16(),
        payload,
    })
}

#[tauri::command]
pub(crate) async fn fetch_launcher_catalog(
    app: AppHandle,
    bridge: tauri::State<'_, BridgeClient>,
    oauth_state: tauri::State<'_, crate::oauth::OauthState>,
    etag: Option<String>,
) -> Result<LabbyHttpResult, String> {
    let settings = merged_settings(&app)?;
    let base_url = validate_saved_server_url(&settings.server_url)?;
    let url = format!("{}/v1/palette/catalog", base_url.trim_end_matches('/'));
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
    response_to_result(response).await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LauncherExecuteRequest {
    id: String,
    params: serde_json::Value,
    confirm_destructive: Option<bool>,
}

#[tauri::command]
pub(crate) async fn execute_launcher_entry(
    app: AppHandle,
    bridge: tauri::State<'_, BridgeClient>,
    oauth_state: tauri::State<'_, crate::oauth::OauthState>,
    request: LauncherExecuteRequest,
) -> Result<LabbyHttpResult, String> {
    validate_launcher_request(&request)?;
    let settings = merged_settings(&app)?;
    let base_url = validate_saved_server_url(&settings.server_url)?;
    let url = format!("{}/v1/palette/execute", base_url.trim_end_matches('/'));
    let client = (*bridge).client();
    let static_token = settings
        .static_token
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let body = serde_json::json!({
        "id": request.id,
        "params": request.params,
        "confirmDestructive": request.confirm_destructive.unwrap_or(false),
    });

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
    response_to_result(response).await
}

async fn response_to_result(response: reqwest::Response) -> Result<LabbyHttpResult, String> {
    let status = response.status();
    if status == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(LabbyHttpResult {
            ok: true,
            status: status.as_u16(),
            payload: serde_json::Value::Null,
        });
    }
    let content_type = response_content_type(&response);
    let text = response.text().await.map_err(|err| err.to_string())?;
    let payload = parse_json_payload(content_type.as_deref(), &text)?;
    Ok(LabbyHttpResult {
        ok: status.is_success(),
        status: status.as_u16(),
        payload,
    })
}

fn response_content_type(response: &reqwest::Response) -> Option<String> {
    response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
}

fn parse_json_payload(content_type: Option<&str>, text: &str) -> Result<serde_json::Value, String> {
    if text.trim().is_empty() {
        return Ok(serde_json::Value::Null);
    }
    let is_json = content_type
        .map(|value| {
            let value = value.to_ascii_lowercase();
            value.starts_with("application/json") || value.contains("+json")
        })
        .unwrap_or(true);
    if !is_json {
        if content_type.is_some_and(|value| value.to_ascii_lowercase().starts_with("text/html")) {
            return Err(WRONG_API_HOST_HINT.to_string());
        }
        return Err(format!(
            "Labby API returned non-JSON content type `{}`",
            content_type.unwrap_or("unknown")
        ));
    }
    serde_json::from_str(text).map_err(|err| format!("Labby API returned invalid JSON: {err}"))
}

fn validate_launcher_request(request: &LauncherExecuteRequest) -> Result<(), String> {
    if request.id.len() > 512 {
        return Err("launcher id must be <= 512 bytes".to_string());
    }
    if !valid_launcher_id(&request.id) {
        return Err(
            "launcher id must be mcp:<upstream>::<tool> or labby:<service>::<action>".to_string(),
        );
    }
    if !request.params.is_object() {
        return Err("launcher params must be a JSON object".to_string());
    }
    let serialized = serde_json::to_vec(&request.params).map_err(|err| err.to_string())?;
    if serialized.len() > 256 * 1024 {
        return Err("launcher params must be <= 256 KiB".to_string());
    }
    if json_depth(&request.params) > 32 {
        return Err("launcher params nesting depth must be <= 32".to_string());
    }
    Ok(())
}

fn valid_launcher_id(id: &str) -> bool {
    fn segment(value: &str) -> bool {
        !value.is_empty()
            && value
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.')
    }
    if let Some(rest) = id.strip_prefix("mcp:") {
        let Some((upstream, tool)) = rest.split_once("::") else {
            return false;
        };
        return segment(upstream) && segment(tool) && !tool.contains("::");
    }
    if let Some(rest) = id.strip_prefix("labby:") {
        let Some((service, action)) = rest.split_once("::") else {
            return false;
        };
        return segment(service) && segment(action) && !action.contains("::");
    }
    false
}

fn json_depth(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Array(values) => 1 + values.iter().map(json_depth).max().unwrap_or(0),
        serde_json::Value::Object(map) => 1 + map.values().map(json_depth).max().unwrap_or(0),
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{LauncherExecuteRequest, parse_json_payload, validate_launcher_request};

    #[test]
    fn validates_launcher_execute_request_shape() {
        validate_launcher_request(&LauncherExecuteRequest {
            id: "mcp:alpha::ping".to_string(),
            params: json!({ "q": "hello" }),
            confirm_destructive: Some(false),
        })
        .expect("valid request");
    }

    #[test]
    fn rejects_invalid_launcher_id_and_non_object_params() {
        assert!(
            validate_launcher_request(&LauncherExecuteRequest {
                id: "../escape".to_string(),
                params: json!({}),
                confirm_destructive: None,
            })
            .is_err()
        );
        assert!(
            validate_launcher_request(&LauncherExecuteRequest {
                id: "mcp:alpha::ping".to_string(),
                params: json!("not-object"),
                confirm_destructive: None,
            })
            .is_err()
        );
    }

    #[test]
    fn rejects_html_payloads_from_web_ui_hosts() {
        let err = parse_json_payload(
            Some("text/html; charset=utf-8"),
            "<!DOCTYPE html><html></html>",
        )
        .expect_err("html response should be rejected");

        assert!(err.contains("LABBY_API_URL"));
    }
}
