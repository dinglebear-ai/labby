//! Surface-neutral browser action dispatch.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use labby_browser::BrowserError;

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{action_schema, help_payload, require_str, to_json};

use super::catalog::ACTIONS;
use super::runtime::browser_bridge;

const MAX_TOOL_TIMEOUT_MS: u64 = 120_000;

#[derive(Deserialize)]
struct CallParams {
    browser_id: String,
    tab_id: i64,
    document_id: String,
    catalog_revision: i64,
    tool_name: String,
    #[serde(default)]
    arguments: Value,
    timeout_ms: Option<u64>,
}

pub async fn dispatch(action: &str, params: Value) -> Result<Value, ToolError> {
    let bridge = browser_bridge().await?;
    match action {
        "help" => Ok(help_payload("browser", ACTIONS)),
        "schema" => action_schema(ACTIONS, require_str(&params, "action")?),
        "browser.status" => Ok(json!({
            "available": true,
            "database": bridge.store().path(),
            "connected_browser_ids": bridge.connected_browser_ids().map_err(map_error)?,
        })),
        "browser.list" => {
            let connected = bridge.connected_browser_ids().map_err(map_error)?;
            let browsers = bridge
                .store()
                .browsers()
                .await
                .map_err(map_error)?
                .into_iter()
                .map(|browser| {
                    let connected = connected.contains(&browser.id);
                    json!({
                        "id": browser.id,
                        "display_name": browser.display_name,
                        "extension_id": browser.extension_id,
                        "paired_at": browser.paired_at,
                        "last_seen_at": browser.last_seen_at,
                        "revoked_at": browser.revoked_at,
                        "connected": connected,
                    })
                })
                .collect::<Vec<_>>();
            Ok(json!({"browsers": browsers}))
        }
        "browser.revoke" => {
            let id = require_str(&params, "browser_id")?;
            to_json(bridge.revoke_browser(&id).await.map_err(map_error)?)
        }
        "browser.pairing.list" => to_json(json!({
            "pairings": bridge.store().pending_pairings().await.map_err(map_error)?
        })),
        "browser.pairing.approve" => {
            let id = require_str(&params, "pairing_id")?;
            to_json(bridge.approve_pairing(&id).await.map_err(map_error)?)
        }
        "browser.sessions" => {
            let cursor = params.get("cursor").and_then(Value::as_str);
            let limit = params
                .get("limit")
                .and_then(Value::as_u64)
                .map(|value| usize::try_from(value).unwrap_or(usize::MAX));
            to_json(
                bridge
                    .store()
                    .sessions(cursor, limit)
                    .await
                    .map_err(map_error)?,
            )
        }
        "browser.session.get" => {
            let id = require_str(&params, "session_id")?;
            to_json(bridge.store().session(&id).await.map_err(map_error)?)
        }
        "browser.session.enable" => {
            let id = require_str(&params, "session_id")?;
            let enabled = params
                .get("enabled")
                .and_then(Value::as_bool)
                .ok_or_else(|| ToolError::InvalidParam {
                    message: "`enabled` must be a boolean".to_string(),
                    param: "enabled".to_string(),
                })?;
            to_json(
                bridge
                    .store()
                    .set_session_enabled(&id, enabled)
                    .await
                    .map_err(map_error)?,
            )
        }
        "browser.call" => {
            let call: CallParams =
                serde_json::from_value(params).map_err(|error| ToolError::InvalidParam {
                    message: format!("invalid browser.call params: {error}"),
                    param: "params".to_string(),
                })?;
            if call
                .timeout_ms
                .is_some_and(|timeout| timeout == 0 || timeout > MAX_TOOL_TIMEOUT_MS)
            {
                return Err(ToolError::InvalidParam {
                    message: format!("`timeout_ms` must be between 1 and {MAX_TOOL_TIMEOUT_MS}"),
                    param: "timeout_ms".to_string(),
                });
            }
            bridge
                .call(
                    &call.browser_id,
                    call.tab_id,
                    call.document_id,
                    call.catalog_revision,
                    call.tool_name,
                    call.arguments,
                    call.timeout_ms.map(Duration::from_millis),
                )
                .await
                .map_err(map_error)
        }
        unknown => Err(ToolError::UnknownAction {
            message: format!("unknown action `{unknown}` for service `browser`"),
            valid: ACTIONS
                .iter()
                .map(|action| action.name.to_string())
                .collect(),
            hint: None,
        }),
    }
}

fn map_error(error: BrowserError) -> ToolError {
    ToolError::Sdk {
        sdk_kind: error.kind().to_string(),
        message: error.to_string(),
    }
}
