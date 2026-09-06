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
    let settings = merged_settings(&app).await?;
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
    let settings = merged_settings(&app).await?;
    let base_url = validate_saved_server_url(&settings.server_url)?;
    let url = format!("{}/v1/{}", base_url.trim_end_matches('/'), request.service);
    let client = (*bridge).client();
    let static_token = settings
        .static_token
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let body = serde_json::json!({ "action": request.action, "params": request.params });
    let project_id = settings.project_id.as_deref();

    let make = |token: Option<&str>| {
        let mut b = client
            .post(&url)
            .header(reqwest::header::ACCEPT, "application/json")
            .json(&body);
        if let Some(t) = token {
            b = b.bearer_auth(t);
        }
        if let Some(project_id) = project_id {
            b = b.header("x-labby-project-id", project_id);
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
    query: Option<String>,
) -> Result<LabbyHttpResult, String> {
    let settings = merged_settings(&app).await?;
    let base_url = validate_saved_server_url(&settings.server_url)?;
    // The desktop palette only renders a bounded shortlist. Send its debounced
    // query so matching occurs before server-side catalog caps; exact schemas
    // are still fetched separately after selection.
    let url = palette_search_url(&base_url, query.as_deref())?;
    let client = (*bridge).client();
    let static_token = settings
        .static_token
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());

    let make = |token: Option<&str>| {
        let mut b = client
            .get(url.as_str())
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
    match response_to_result(response).await {
        Ok(result) => Ok(result),
        Err(err) if err == WRONG_API_HOST_HINT => {
            let discovered = discover_api_base_url(client, &base_url).await?;
            let url = palette_search_url(&discovered, query.as_deref())?;
            let make = |token: Option<&str>| {
                let mut b = client
                    .get(url.as_str())
                    .header(reqwest::header::ACCEPT, "application/json");
                if let Some(t) = token {
                    b = b.bearer_auth(t);
                }
                if let Some(etag) = &etag {
                    b = b.header(reqwest::header::IF_NONE_MATCH, etag);
                }
                b
            };
            let response = crate::oauth::send_with_reauth(
                &app,
                client,
                &discovered,
                static_token,
                &oauth_state,
                make,
            )
            .await?;
            response_to_result(response).await
        }
        Err(err) => Err(err),
    }
}

fn palette_search_url(base_url: &str, query: Option<&str>) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(&format!(
        "{}/v1/palette/search",
        base_url.trim_end_matches('/')
    ))
    .map_err(|error| format!("invalid palette search URL: {error}"))?;
    url.query_pairs_mut()
        .append_pair("q", query.unwrap_or_default())
        .append_pair("limit", "100");
    Ok(url)
}

#[tauri::command]
pub(crate) async fn fetch_launcher_schema(
    app: AppHandle,
    bridge: tauri::State<'_, BridgeClient>,
    oauth_state: tauri::State<'_, crate::oauth::OauthState>,
    id: String,
) -> Result<LabbyHttpResult, String> {
    if id.len() > 512 || !valid_launcher_id(&id) {
        return Err(
            "launcher id must be mcp:<upstream>::<tool> or labby:<service>::<action>".to_string(),
        );
    }
    let settings = merged_settings(&app).await?;
    let base_url = validate_saved_server_url(&settings.server_url)?;
    let mut url = reqwest::Url::parse(&format!(
        "{}/v1/palette/schema",
        base_url.trim_end_matches('/')
    ))
    .map_err(|err| err.to_string())?;
    url.query_pairs_mut().append_pair("id", &id);
    let client = (*bridge).client();
    let static_token = settings
        .static_token
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());

    let make = |token: Option<&str>| {
        let mut b = client
            .get(url.clone())
            .header(reqwest::header::ACCEPT, "application/json");
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LauncherExecuteRequest {
    id: String,
    params: serde_json::Value,
    confirm_destructive: Option<bool>,
    expected_contract_hash: String,
}

#[tauri::command]
pub(crate) async fn execute_launcher_entry(
    app: AppHandle,
    bridge: tauri::State<'_, BridgeClient>,
    oauth_state: tauri::State<'_, crate::oauth::OauthState>,
    request: LauncherExecuteRequest,
) -> Result<LabbyHttpResult, String> {
    validate_launcher_request(&request)?;
    let settings = merged_settings(&app).await?;
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
        "expectedContractHash": request.expected_contract_hash,
    });
    let project_id = settings.project_id.as_deref();

    let make = |token: Option<&str>| {
        let mut b = client
            .post(&url)
            .header(reqwest::header::ACCEPT, "application/json")
            .json(&body);
        if let Some(t) = token {
            b = b.bearer_auth(t);
        }
        if let Some(project_id) = project_id {
            b = b.header("x-labby-project-id", project_id);
        }
        b
    };
    let response =
        crate::oauth::send_with_reauth(&app, client, &base_url, static_token, &oauth_state, make)
            .await?;
    match response_to_result(response).await {
        Ok(result) => Ok(result),
        Err(err) if err == WRONG_API_HOST_HINT => {
            let discovered = discover_api_base_url(client, &base_url).await?;
            let url = format!("{}/v1/palette/execute", discovered.trim_end_matches('/'));
            let make = |token: Option<&str>| {
                let mut b = client
                    .post(&url)
                    .header(reqwest::header::ACCEPT, "application/json")
                    .json(&body);
                if let Some(t) = token {
                    b = b.bearer_auth(t);
                }
                if let Some(project_id) = project_id {
                    b = b.header("x-labby-project-id", project_id);
                }
                b
            };
            let response = crate::oauth::send_with_reauth(
                &app,
                client,
                &discovered,
                static_token,
                &oauth_state,
                make,
            )
            .await?;
            response_to_result(response).await
        }
        Err(err) => Err(err),
    }
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

async fn discover_api_base_url(client: &reqwest::Client, base_url: &str) -> Result<String, String> {
    let url = format!("{}/.well-known/labby.json", base_url.trim_end_matches('/'));
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|err| format!("{WRONG_API_HOST_HINT}; discovery failed: {err}"))?;
    let content_type = response_content_type(&response);
    let text = response.text().await.map_err(|err| err.to_string())?;
    let payload = parse_json_payload(content_type.as_deref(), &text)
        .map_err(|err| format!("{WRONG_API_HOST_HINT}; discovery failed: {err}"))?;
    let api_base = payload
        .get("apiBaseUrl")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{WRONG_API_HOST_HINT}; discovery omitted apiBaseUrl"))?;
    validate_trusted_discovered_api_base_url(base_url, api_base)
}

fn validate_discovered_api_base_url(value: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(value)
        .map_err(|err| format!("{WRONG_API_HOST_HINT}; discovery apiBaseUrl is invalid: {err}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(format!(
            "{WRONG_API_HOST_HINT}; discovery apiBaseUrl must be an http(s) origin"
        ));
    }
    Ok(url.origin().ascii_serialization())
}

/// Treat the configured server authority as the credential trust anchor.
/// Discovery may select a path, but it must not redirect OAuth or static
/// bearer credentials to an authority that the operator did not configure.
fn validate_trusted_discovered_api_base_url(
    configured_base_url: &str,
    discovered_api_base_url: &str,
) -> Result<String, String> {
    let configured = reqwest::Url::parse(configured_base_url)
        .map_err(|err| format!("configured Labby server URL is invalid: {err}"))?;
    let discovered_origin = validate_discovered_api_base_url(discovered_api_base_url)?;
    let discovered = reqwest::Url::parse(&discovered_origin)
        .map_err(|err| format!("{WRONG_API_HOST_HINT}; discovery apiBaseUrl is invalid: {err}"))?;

    if configured.origin() != discovered.origin() {
        return Err(format!(
            "{WRONG_API_HOST_HINT}; discovery apiBaseUrl must use the explicitly trusted origin"
        ));
    }

    let loopback = discovered.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if discovered.scheme() != "https" && !loopback {
        return Err(format!(
            "{WRONG_API_HOST_HINT}; discovery apiBaseUrl must use TLS outside loopback"
        ));
    }

    Ok(discovered_origin)
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
    if request.expected_contract_hash.len() != 64
        || !request
            .expected_contract_hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(
            "expected contract hash must be 64 lowercase hexadecimal characters".to_string(),
        );
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

    use super::{
        LauncherExecuteRequest, palette_search_url, parse_json_payload,
        validate_discovered_api_base_url, validate_launcher_request,
        validate_trusted_discovered_api_base_url,
    };

    #[test]
    fn validates_launcher_execute_request_shape() {
        validate_launcher_request(&LauncherExecuteRequest {
            id: "mcp:alpha::ping".to_string(),
            params: json!({ "q": "hello" }),
            confirm_destructive: Some(false),
            expected_contract_hash: "a".repeat(64),
        })
        .expect("valid request");
    }

    #[test]
    fn rejects_missing_or_malformed_launcher_contract_hash() {
        for expected_contract_hash in [String::new(), "A".repeat(64), "a".repeat(63)] {
            assert!(
                validate_launcher_request(&LauncherExecuteRequest {
                    id: "mcp:alpha::ping".to_string(),
                    params: json!({}),
                    confirm_destructive: None,
                    expected_contract_hash,
                })
                .is_err()
            );
        }
    }

    #[test]
    fn rejects_invalid_launcher_id_and_non_object_params() {
        assert!(
            validate_launcher_request(&LauncherExecuteRequest {
                id: "../escape".to_string(),
                params: json!({}),
                confirm_destructive: None,
                expected_contract_hash: "a".repeat(64),
            })
            .is_err()
        );
        assert!(
            validate_launcher_request(&LauncherExecuteRequest {
                id: "mcp:alpha::ping".to_string(),
                params: json!("not-object"),
                confirm_destructive: None,
                expected_contract_hash: "a".repeat(64),
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

    #[test]
    fn validates_discovered_api_base_url_as_http_origin() {
        assert_eq!(
            validate_discovered_api_base_url("https://api.example.com/path").unwrap(),
            "https://api.example.com"
        );
        assert!(validate_discovered_api_base_url("file:///tmp/labby").is_err());
    }

    #[test]
    fn palette_search_url_encodes_query_for_primary_and_discovered_origins() {
        for base in ["https://labby.example.com", "https://labby.example.com/"] {
            let url = palette_search_url(base, Some("owner: foo/bar & baz"))
                .expect("valid palette search URL");
            assert_eq!(url.path(), "/v1/palette/search");
            assert_eq!(
                url.query_pairs()
                    .map(|(key, value)| (key.into_owned(), value.into_owned()))
                    .collect::<Vec<_>>(),
                [
                    ("q".to_string(), "owner: foo/bar & baz".to_string()),
                    ("limit".to_string(), "100".to_string()),
                ]
            );
        }
    }

    #[test]
    fn rejects_attacker_api_base_url_before_forwarding_credentials() {
        let error = validate_trusted_discovered_api_base_url(
            "https://labby.example.com",
            "https://attacker.example/api",
        )
        .expect_err("cross-origin discovery must not become a credential target");

        assert!(error.contains("trusted origin"));
    }

    #[test]
    fn discovered_credential_target_requires_tls_outside_loopback() {
        assert_eq!(
            validate_trusted_discovered_api_base_url(
                "https://labby.example.com",
                "https://labby.example.com/api",
            )
            .unwrap(),
            "https://labby.example.com"
        );
        assert!(
            validate_trusted_discovered_api_base_url(
                "http://labby.example.com",
                "http://labby.example.com/api",
            )
            .expect_err("plaintext remote credential target must be rejected")
            .contains("TLS")
        );
        assert_eq!(
            validate_trusted_discovered_api_base_url(
                "http://127.0.0.1:8765",
                "http://127.0.0.1:8765/api",
            )
            .unwrap(),
            "http://127.0.0.1:8765"
        );
        assert!(
            validate_trusted_discovered_api_base_url(
                "https://127.0.0.1:8765",
                "http://127.0.0.1:8765/api",
            )
            .expect_err("HTTPS trust anchor must not downgrade to HTTP on loopback")
            .contains("trusted origin")
        );
    }
}
