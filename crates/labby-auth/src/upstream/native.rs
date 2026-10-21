//! Bounded client for Labby's server-hosted native OAuth handoff.
//!
//! The caller owns browser presentation, polling cadence and the overall deadline.
use std::time::Duration;

use oauth2::http::{Request, StatusCode, header};
use rmcp_client::transport::auth::AuthorizationMetadata;
use serde::Deserialize;
use url::Url;

use super::http_client::TrustedOriginOAuthHttpClient;
use super::types::{OAuthEgressKind, OauthError};

const START_MEDIA_TYPE: &str = "application/vnd.labby.native-oauth-start+json";
const MAX_BODY: usize = 64 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// A fully validated native extension bound to the selected server origin.
/// This type deliberately does not implement `Debug`.
pub struct NativeAuthorization {
    client: TrustedOriginOAuthHttpClient,
    origin: Url,
    callback: Url,
    poll: Url,
    issuer: String,
}

/// Ephemeral handoff state. The polling credential is never exposed or formatted.
pub struct NativeAuthorizationSession {
    native: NativeAuthorization,
    authorization_url: String,
    poll_token: String,
}

fn invalid(message: &str) -> OauthError {
    OauthError::Egress {
        kind: OAuthEgressKind::ValidationFailed,
        message: message.into(),
    }
}
fn protocol(message: &str) -> OauthError {
    OauthError::Egress {
        kind: OAuthEgressKind::UpstreamError,
        message: message.into(),
    }
}
fn safe_url(value: &str) -> Result<Url, OauthError> {
    let url = Url::parse(value).map_err(|_| invalid("invalid native OAuth URL"))?;
    let loopback = match url.host() {
        Some(url::Host::Domain("localhost")) => true,
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    };
    if !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
    {
        return Err(invalid("unsafe native OAuth URL"));
    }
    Ok(url)
}
fn endpoint(value: &str, origin: &Url) -> Result<Url, OauthError> {
    let url = safe_url(value)?;
    if url.origin() != origin.origin() || url.query().is_some() {
        return Err(invalid(
            "native OAuth endpoint does not match selected server origin",
        ));
    }
    Ok(url)
}

impl NativeAuthorization {
    /// Recognize the v2 extension only when every advertised field is valid.
    pub fn discover(
        server_url: &str,
        metadata: &AuthorizationMetadata,
    ) -> Result<Option<Self>, OauthError> {
        let keys = [
            "native_callback_endpoint",
            "native_poll_endpoint_v2",
            "native_authorization_start_media_type",
        ];
        if keys
            .iter()
            .all(|key| !metadata.additional_fields.contains_key(*key))
        {
            return Ok(None);
        }
        let field = |key: &str| {
            metadata
                .additional_fields
                .get(key)
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| invalid("incomplete native OAuth extension"))
        };
        let origin = safe_url(server_url)?;
        let callback = endpoint(field(keys[0])?, &origin)?;
        let poll = endpoint(field(keys[1])?, &origin)?;
        if field(keys[2])? != START_MEDIA_TYPE {
            return Err(invalid("unsupported native OAuth start media type"));
        }
        let issuer = metadata
            .issuer
            .as_deref()
            .ok_or_else(|| invalid("native OAuth issuer missing"))?;
        endpoint(issuer, &origin)?;
        endpoint(&metadata.authorization_endpoint, &origin)?;
        endpoint(&metadata.token_endpoint, &origin)?;
        let client = TrustedOriginOAuthHttpClient::new(server_url)?;
        Ok(Some(Self {
            client,
            origin,
            callback,
            poll,
            issuer: issuer.into(),
        }))
    }

    /// Server callback URI to register with the existing authorization manager.
    pub fn callback_uri(&self) -> &str {
        self.callback.as_str()
    }
    /// Issuer proven to share the selected server and extension's exact origin.
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// Negotiate a native handoff without following the provider redirect.
    pub async fn start(
        self,
        authorize_url: &str,
    ) -> Result<NativeAuthorizationSession, OauthError> {
        let url = safe_url(authorize_url)?;
        if url.origin() != self.origin.origin() {
            return Err(invalid("native OAuth authorization origin mismatch"));
        }
        let request = Request::builder()
            .uri(url.as_str())
            .header(header::ACCEPT, START_MEDIA_TYPE)
            .body(Vec::new())
            .map_err(|_| invalid("invalid native OAuth request"))?;
        let response = self.execute(request).await?;
        if response.status() != StatusCode::OK {
            return Err(protocol("native OAuth start returned unexpected status"));
        }
        #[derive(Deserialize)]
        struct Start {
            authorization_url: String,
            poll_token: String,
        }
        let start: Start = decode(response)?;
        let authorization_url = safe_url(&start.authorization_url)?.to_string();
        if start.poll_token.is_empty() || start.poll_token.len() > 4096 {
            return Err(protocol("invalid native OAuth polling credential"));
        }
        Ok(NativeAuthorizationSession {
            native: self,
            authorization_url,
            poll_token: start.poll_token,
        })
    }

    async fn execute(
        &self,
        request: oauth2::HttpRequest,
    ) -> Result<oauth2::HttpResponse, OauthError> {
        tokio::time::timeout(REQUEST_TIMEOUT, self.client.execute_native(request))
            .await
            .map_err(|_| OauthError::Egress {
                kind: OAuthEgressKind::Timeout,
                message: "native OAuth request timed out".into(),
            })?
            .map_err(|_| protocol("native OAuth HTTP request failed"))
    }
}

fn decode<T: serde::de::DeserializeOwned>(response: oauth2::HttpResponse) -> Result<T, OauthError> {
    if response.body().len() > MAX_BODY {
        return Err(OauthError::Egress {
            kind: OAuthEgressKind::ResponseTooLarge,
            message: "native OAuth response exceeded size limit".into(),
        });
    }
    let json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.split(';').next())
        .is_some_and(|h| h.trim().eq_ignore_ascii_case("application/json"));
    if !json {
        return Err(protocol("native OAuth response requires JSON content type"));
    }
    serde_json::from_slice(response.body())
        .map_err(|_| protocol("invalid native OAuth JSON response"))
}

impl NativeAuthorizationSession {
    /// Provider URL for the operator to open in their browser.
    pub fn authorization_url(&self) -> &str {
        &self.authorization_url
    }
    /// Poll once. Dropping this future cancels its request; no retries are hidden.
    pub async fn poll(&self) -> Result<Option<String>, OauthError> {
        let body = serde_json::to_vec(&serde_json::json!({"poll_token": self.poll_token}))
            .map_err(|_| protocol("native OAuth request encoding failed"))?;
        let request = Request::builder()
            .method("POST")
            .uri(self.native.poll.as_str())
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json")
            .body(body)
            .map_err(|_| invalid("invalid native OAuth polling request"))?;
        let response = self.native.execute(request).await?;
        let status = response.status();
        if status != StatusCode::OK && status != StatusCode::ACCEPTED {
            return Err(protocol("native OAuth poll returned unexpected status"));
        }
        #[derive(Deserialize)]
        struct Poll {
            code: Option<String>,
        }
        let poll: Poll = decode(response)?;
        match (status, poll.code) {
            (StatusCode::ACCEPTED, None) => Ok(None),
            (StatusCode::OK, Some(code)) if !code.is_empty() && code.len() <= 4096 => {
                Ok(Some(code))
            }
            _ => Err(protocol("native OAuth poll returned inconsistent result")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn metadata(origin: &str) -> AuthorizationMetadata {
        serde_json::from_value(serde_json::json!({
            "issuer": origin,
            "authorization_endpoint": format!("{origin}/authorize"),
            "token_endpoint": format!("{origin}/token"),
            "native_callback_endpoint": format!("{origin}/native/callback"),
            "native_poll_endpoint_v2": format!("{origin}/native/poll"),
            "native_authorization_start_media_type": START_MEDIA_TYPE
        }))
        .unwrap()
    }
    #[test]
    fn absence_is_optional_but_partial_or_untrusted_extensions_fail_closed() {
        let mut absent = metadata("https://example.test");
        absent.additional_fields.clear();
        assert!(
            NativeAuthorization::discover("https://example.test/mcp", &absent)
                .unwrap()
                .is_none()
        );
        absent.additional_fields.insert(
            "native_poll_endpoint_v2".into(),
            serde_json::json!("https://example.test/native/poll"),
        );
        assert!(NativeAuthorization::discover("https://example.test/mcp", &absent).is_err());
        for bad in [
            "https://evil.test/poll",
            "https://user@example.test/poll",
            "https://example.test/poll?secret=1",
            "https://example.test/poll#fragment",
            "http://example.test/poll",
        ] {
            let mut m = metadata("https://example.test");
            m.additional_fields
                .insert("native_poll_endpoint_v2".into(), serde_json::json!(bad));
            assert!(
                NativeAuthorization::discover("https://example.test/mcp", &m).is_err(),
                "{bad}"
            );
        }
        let mut m = metadata("https://example.test");
        m.issuer = Some("https://evil.test".into());
        assert!(NativeAuthorization::discover("https://example.test/mcp", &m).is_err());
    }
    #[tokio::test]
    async fn cross_origin_authorization_is_rejected_before_egress() {
        let native = NativeAuthorization::discover(
            "https://example.test/mcp",
            &metadata("https://example.test"),
        )
        .unwrap()
        .unwrap();
        let result = native
            .start("https://other.invalid/authorize?state=secret")
            .await;
        let error = result.err().expect("cross-origin authorization rejected");
        assert_eq!(error.kind(), "validation_failed");
        assert!(!error.to_string().contains("secret"));
    }

    #[test]
    fn response_contract_rejects_invalid_json_content_type_and_oversize() {
        for (content_type, body) in [
            ("text/plain", b"{}".to_vec()),
            ("application/json", b"invalid".to_vec()),
            ("application/json", vec![b' '; MAX_BODY + 1]),
        ] {
            let response = oauth2::http::Response::builder()
                .header(header::CONTENT_TYPE, content_type)
                .body(body)
                .unwrap();
            assert!(decode::<serde_json::Value>(response).is_err());
        }
    }
    #[cfg(feature = "http-axum")]
    #[tokio::test]
    async fn native_start_and_poll_keep_secret_in_post_body_and_do_not_follow_redirects() {
        use axum::{
            Json, Router,
            extract::State,
            http::{HeaderMap, Uri},
            routing::{get, post},
        };
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/authorize", get(|headers: HeaderMap| async move {
                assert_eq!(headers.get(header::ACCEPT).unwrap(), START_MEDIA_TYPE);
                Json(serde_json::json!({"authorization_url":"https://provider.example/authorize?state=public", "poll_token":"private-poll-secret"}))
            }))
            .route("/native/poll", post(|State(calls): State<Arc<AtomicUsize>>, uri: Uri, Json(body): Json<serde_json::Value>| async move {
                assert!(uri.query().is_none());
                assert_eq!(body, serde_json::json!({"poll_token":"private-poll-secret"}));
                if calls.fetch_add(1, Ordering::SeqCst) == 0 { (StatusCode::ACCEPTED, Json(serde_json::json!({"code":null}))) }
                else { (StatusCode::OK, Json(serde_json::json!({"code":"authorization-code"}))) }
            }))
            .route("/redirect", get(|| async { (StatusCode::FOUND, [(header::LOCATION, "https://evil.example/steal")]) }))
            .route("/expired", post(|| async { (StatusCode::BAD_REQUEST, "private-poll-secret") }))
            .route("/malformed", post(|| async { Json(serde_json::json!({"code":null})) }))
            .route("/slow", post(|| async { std::future::pending::<String>().await }))
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let native = NativeAuthorization::discover(&format!("{origin}/mcp"), &metadata(&origin))
            .unwrap()
            .unwrap();
        let session = native
            .start(&format!("{origin}/authorize?state=manager-state"))
            .await
            .unwrap();
        assert_eq!(session.poll().await.unwrap(), None);
        assert_eq!(
            session.poll().await.unwrap().as_deref(),
            Some("authorization-code")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let native = NativeAuthorization::discover(&origin, &metadata(&origin))
            .unwrap()
            .unwrap();
        let error = native
            .start(&format!("{origin}/redirect"))
            .await
            .err()
            .expect("redirect rejected");
        assert!(!error.to_string().contains("private-poll-secret"));
        for path in ["expired", "malformed"] {
            let mut m = metadata(&origin);
            m.additional_fields.insert(
                "native_poll_endpoint_v2".into(),
                serde_json::json!(format!("{origin}/{path}")),
            );
            let native = NativeAuthorization::discover(&origin, &m).unwrap().unwrap();
            let session = native.start(&format!("{origin}/authorize")).await.unwrap();
            let error = session.poll().await.unwrap_err();
            assert!(!error.to_string().contains("private-poll-secret"));
        }
        let mut m = metadata(&origin);
        m.additional_fields.insert(
            "native_poll_endpoint_v2".into(),
            serde_json::json!(format!("{origin}/slow")),
        );
        let native = NativeAuthorization::discover(&origin, &m).unwrap().unwrap();
        let session = native.start(&format!("{origin}/authorize")).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), session.poll())
                .await
                .is_err()
        );
        // The canceled operation does not leave a hidden retry loop: another poll
        // can be independently canceled by the owning caller as well.
        assert!(
            tokio::time::timeout(Duration::from_millis(20), session.poll())
                .await
                .is_err()
        );
        server.abort();
    }
}
