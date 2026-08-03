//! Bounded HTTP side-channel messages used by relayed tool cancellation.

use std::collections::HashMap;
use std::sync::Arc;

use reqwest::header::{HeaderName, HeaderValue};
use rmcp::model::{
    CancelledNotification, CancelledNotificationParam, ClientCapabilities, ClientJsonRpcMessage,
    ClientNotification, ClientRequest, CustomRequest, CustomResult, GetMeta, JsonRpcMessage,
    NotificationMetaObject, ProtocolVersion, RequestId, ServerResult,
};
use rmcp::transport::AuthClient;
use rmcp::transport::common::http_header::{HEADER_MCP_METHOD, HEADER_MCP_PROTOCOL_VERSION};
use rmcp::transport::streamable_http_client::{StreamableHttpClient, StreamableHttpPostResponse};

use labby_auth::upstream::cache::OauthClientCache;
use labby_runtime::gateway_config::{UpstreamConfig, UpstreamTransport};

use crate::{MCP_RELAY_CANCELLATION_REQUEST_METHOD, MCP_RELAY_CANCELLATION_TOKEN_META_KEY};

use super::super::auth::configured_bearer_token;
use super::super::http_client;
use super::connect::configured_custom_headers;
#[cfg(unix)]
use super::connect::unix_socket_connect_path;
use super::helpers::{DEFAULT_REQUEST_TIMEOUT, max_response_bytes};

#[derive(Clone)]
enum HttpCancellationClient {
    Plain(http_client::BodyCappedHttpClient),
    Oauth(AuthClient<http_client::BodyCappedHttpClient>),
}

/// Sends explicit cancellation messages for HTTP and Unix-socket transports.
#[derive(Clone)]
pub(super) struct HttpCancellationSender {
    uri: Arc<str>,
    client: HttpCancellationClient,
    auth_token: Option<String>,
    custom_headers: HashMap<HeaderName, HeaderValue>,
}

fn relay_cancellation_request(reason: &str, token: &str) -> ClientJsonRpcMessage {
    let mut request = CustomRequest::new(
        MCP_RELAY_CANCELLATION_REQUEST_METHOD,
        Some(serde_json::json!({
            "reason": reason,
            "token": token,
        })),
    );
    request
        .get_meta_mut()
        .set_protocol_version(ProtocolVersion::V_2026_07_28);
    request
        .get_meta_mut()
        .set_client_capabilities(ClientCapabilities::default());
    ClientJsonRpcMessage::request(
        ClientRequest::CustomRequest(request),
        RequestId::String(Arc::from(format!("relay-cancel:{token}"))),
    )
}

fn cancellation_message(
    request_id: RequestId,
    reason: Option<String>,
    token: &str,
) -> ClientJsonRpcMessage {
    let mut params = CancelledNotificationParam::new(Some(request_id), reason);
    let mut meta = NotificationMetaObject::new();
    meta.0.0.insert(
        MCP_RELAY_CANCELLATION_TOKEN_META_KEY.to_string(),
        serde_json::Value::String(token.to_string()),
    );
    params.meta = Some(meta.clone());
    let mut cancelled = CancelledNotification::new(params);
    cancelled.extensions.insert(meta);
    ClientJsonRpcMessage::notification(ClientNotification::CancelledNotification(cancelled))
}

fn cancellation_headers_for_message(
    base: &HashMap<HeaderName, HeaderValue>,
    message: &ClientJsonRpcMessage,
) -> anyhow::Result<HashMap<HeaderName, HeaderValue>> {
    let wire = serde_json::to_value(message)?;
    let method = wire
        .get("method")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("HTTP cancellation message omitted its MCP method"))?;
    let mut headers = base.clone();
    headers.insert(
        HeaderName::from_bytes(HEADER_MCP_METHOD.as_bytes())?,
        HeaderValue::from_str(method)?,
    );
    Ok(headers)
}

impl HttpCancellationSender {
    async fn post_message(
        &self,
        message: ClientJsonRpcMessage,
    ) -> anyhow::Result<StreamableHttpPostResponse> {
        let custom_headers = cancellation_headers_for_message(&self.custom_headers, &message)?;
        let result = match &self.client {
            HttpCancellationClient::Plain(client) => {
                client
                    .post_message(
                        Arc::clone(&self.uri),
                        message,
                        None,
                        self.auth_token.clone(),
                        custom_headers,
                    )
                    .await
            }
            HttpCancellationClient::Oauth(client) => {
                client
                    .post_message(Arc::clone(&self.uri), message, None, None, custom_headers)
                    .await
            }
        };
        result.map_err(|error| anyhow::anyhow!("explicit HTTP cancellation failed: {error}"))
    }

    pub(super) async fn send_relay_token(&self, reason: &str, token: &str) -> anyhow::Result<bool> {
        let response = self
            .post_message(relay_cancellation_request(reason, token))
            .await?;
        let message = match response {
            StreamableHttpPostResponse::Accepted => return Ok(false),
            StreamableHttpPostResponse::Json(message, _) => message,
            response => {
                response
                    .expect_initialized::<reqwest::Error>()
                    .await
                    .map_err(|error| {
                        anyhow::anyhow!("invalid HTTP relay cancellation response: {error}")
                    })?
                    .0
            }
        };
        let JsonRpcMessage::Response(response) = message else {
            anyhow::bail!("HTTP relay cancellation did not return a JSON-RPC response");
        };
        let ServerResult::CustomResult(CustomResult(result)) = response.result else {
            anyhow::bail!("HTTP relay cancellation returned an unexpected result type");
        };
        result
            .get("cancelled")
            .and_then(serde_json::Value::as_bool)
            .ok_or_else(|| {
                anyhow::anyhow!("HTTP relay cancellation response omitted boolean `cancelled`")
            })
    }

    pub(super) async fn send(
        &self,
        request_id: RequestId,
        reason: Option<String>,
        token: &str,
    ) -> anyhow::Result<()> {
        self.post_message(cancellation_message(request_id, reason, token))
            .await
            .map(drop)
    }
}

/// Build the side channel for transports where rmcp does not transmit a
/// cancelled notification when its local request handle is closed.
pub(super) async fn build_http_cancellation_sender(
    config: &UpstreamConfig,
    subject: Option<&str>,
    oauth_client_cache: Option<&OauthClientCache>,
    shared_client: Option<&reqwest::Client>,
) -> anyhow::Result<Option<HttpCancellationSender>> {
    let transport = config.effective_transport();
    if !matches!(
        transport,
        Some(UpstreamTransport::Http | UpstreamTransport::UnixSocket)
    ) {
        return Ok(None);
    }

    let url = config.url.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "upstream {} HTTP cancellation sender has no url",
            config.name
        )
    })?;
    let mut custom_headers = configured_custom_headers(config)?;
    custom_headers.insert(
        HeaderName::from_bytes(HEADER_MCP_PROTOCOL_VERSION.as_bytes())?,
        HeaderValue::from_str(&ProtocolVersion::V_2026_07_28.to_string())?,
    );

    let base_client = match transport {
        Some(UpstreamTransport::Http) => match shared_client {
            Some(client) => client.clone(),
            None => {
                drop(rustls::crypto::ring::default_provider().install_default());
                reqwest::Client::builder()
                    .timeout(DEFAULT_REQUEST_TIMEOUT)
                    .build()?
            }
        },
        Some(UpstreamTransport::UnixSocket) => {
            #[cfg(unix)]
            {
                let socket_path = config.socket_path.as_deref().ok_or_else(|| {
                    anyhow::anyhow!(
                        "upstream {} Unix socket cancellation sender has no socket_path",
                        config.name
                    )
                })?;
                drop(rustls::crypto::ring::default_provider().install_default());
                reqwest::Client::builder()
                    .timeout(DEFAULT_REQUEST_TIMEOUT)
                    .http1_only()
                    .unix_socket(unix_socket_connect_path(socket_path))
                    .build()?
            }
            #[cfg(not(unix))]
            {
                return Ok(None);
            }
        }
        _ => return Ok(None),
    };
    let capped = http_client::BodyCappedHttpClient::new(base_client, max_response_bytes());

    let (client, auth_token) = if config.oauth.is_some() {
        let subject = subject.ok_or_else(|| {
            anyhow::anyhow!(
                "upstream {} requires an authenticated subject for cancellation",
                config.name
            )
        })?;
        let cache = oauth_client_cache.ok_or_else(|| {
            anyhow::anyhow!(
                "upstream {} requires OAuth but no auth client cache is registered",
                config.name
            )
        })?;
        let auth_client = cache
            .get_or_build_capped(config, subject, capped)
            .await
            .map_err(|error| anyhow::anyhow!("oauth_required: {error}"))?;
        (HttpCancellationClient::Oauth(auth_client), None)
    } else {
        let auth_token = config
            .bearer_token_env
            .as_deref()
            .and_then(configured_bearer_token);
        (HttpCancellationClient::Plain(capped), auth_token)
    };

    Ok(Some(HttpCancellationSender {
        uri: Arc::from(url),
        client,
        auth_token,
        custom_headers,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_token_cancellation_uses_a_request_for_stateless_http_dispatch() {
        let message = relay_cancellation_request(
            "downstream request cancelled",
            "test-relay-cancellation-token",
        );
        let wire = serde_json::to_value(&message).expect("serialize relay cancellation request");
        let base_headers = HashMap::from([(
            HeaderName::from_bytes(HEADER_MCP_PROTOCOL_VERSION.as_bytes())
                .expect("protocol header name"),
            HeaderValue::from_static("2026-07-28"),
        )]);
        let headers = cancellation_headers_for_message(&base_headers, &message)
            .expect("build relay cancellation headers");

        assert!(matches!(message, ClientJsonRpcMessage::Request(_)));
        assert_eq!(
            wire.get("method").and_then(serde_json::Value::as_str),
            Some(MCP_RELAY_CANCELLATION_REQUEST_METHOD)
        );
        assert_eq!(
            wire.pointer("/params/token")
                .and_then(serde_json::Value::as_str),
            Some("test-relay-cancellation-token")
        );
        assert_eq!(
            wire.pointer("/params/reason")
                .and_then(serde_json::Value::as_str),
            Some("downstream request cancelled")
        );
        assert_eq!(
            wire.pointer("/params/_meta/io.modelcontextprotocol~1protocolVersion")
                .and_then(serde_json::Value::as_str),
            Some(ProtocolVersion::V_2026_07_28.as_str())
        );
        assert!(
            wire.pointer("/params/_meta/io.modelcontextprotocol~1clientCapabilities")
                .is_some()
        );
        assert_eq!(
            headers
                .get(
                    &HeaderName::from_bytes(HEADER_MCP_METHOD.as_bytes())
                        .expect("MCP method header name"),
                )
                .and_then(|value| value.to_str().ok()),
            Some(MCP_RELAY_CANCELLATION_REQUEST_METHOD)
        );
        assert_eq!(
            headers
                .get(
                    &HeaderName::from_bytes(HEADER_MCP_PROTOCOL_VERSION.as_bytes())
                        .expect("MCP protocol header name"),
                )
                .and_then(|value| value.to_str().ok()),
            Some(ProtocolVersion::V_2026_07_28.as_str())
        );
        assert!(wire.get("id").is_some());
    }

    #[test]
    fn cancellation_token_survives_json_round_trip() {
        let token = "test-cancellation-token";
        let message = cancellation_message(
            RequestId::Number(13),
            Some("downstream request cancelled".to_string()),
            token,
        );
        let wire = serde_json::to_value(&message).expect("serialize cancellation notification");
        let decoded: ClientJsonRpcMessage =
            serde_json::from_value(wire).expect("deserialize cancellation notification");
        assert!(
            matches!(&decoded, ClientJsonRpcMessage::Notification(_)),
            "expected cancellation notification"
        );
        let notification = match decoded {
            ClientJsonRpcMessage::Notification(notification) => notification,
            _ => return,
        };
        assert!(
            matches!(
                &notification.notification,
                ClientNotification::CancelledNotification(_)
            ),
            "expected cancelled notification"
        );
        let cancelled = match notification.notification {
            ClientNotification::CancelledNotification(cancelled) => cancelled,
            _ => return,
        };
        let typed_token = cancelled.params.meta.as_ref().and_then(|meta| {
            meta.0
                .0
                .get(MCP_RELAY_CANCELLATION_TOKEN_META_KEY)
                .and_then(serde_json::Value::as_str)
        });
        let extension_token = cancelled
            .extensions
            .get::<NotificationMetaObject>()
            .and_then(|meta| {
                meta.0
                    .0
                    .get(MCP_RELAY_CANCELLATION_TOKEN_META_KEY)
                    .and_then(serde_json::Value::as_str)
            });
        assert_eq!(typed_token.or(extension_token), Some(token));
    }
}
