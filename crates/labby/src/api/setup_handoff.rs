//! Local installer -> browser handoff. No long-lived bearer enters the browser.
use super::state::AppState;
use axum::{
    Extension, Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::net::SocketAddr;

fn response(status: StatusCode, body: Value) -> Response {
    (
        status,
        [
            (header::CACHE_CONTROL, "private, no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        Json(body),
    )
        .into_response()
}
fn denied() -> Response {
    response(
        StatusCode::FORBIDDEN,
        json!({"kind":"forbidden","message":"Local setup handoff could not be verified. Run local setup again."}),
    )
}

fn local_origin(peer: Option<SocketAddr>, headers: &HeaderMap) -> Option<String> {
    if !peer.is_some_and(|peer| peer.ip().is_loopback())
        || headers
            .keys()
            .any(|key| key.as_str() == "forwarded" || key.as_str().starts_with("x-forwarded-"))
    {
        return None;
    }
    let host = headers.get(header::HOST)?.to_str().ok()?;
    if host.chars().any(|character| {
        character.is_whitespace() || matches!(character, '/' | '\\' | '@' | '?' | '#')
    }) {
        return None;
    }
    let url = url::Url::parse(&format!("http://{host}")).ok()?;
    if url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    match url.host()? {
        url::Host::Ipv4(ip) if ip.is_loopback() => {}
        url::Host::Ipv6(ip) if ip.is_loopback() => {}
        _ => return None,
    }
    Some(url.origin().ascii_serialization())
}

pub(super) async fn start(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
) -> Response {
    let Some(origin) = local_origin(peer.map(|Extension(ConnectInfo(peer))| peer), &headers) else {
        return denied();
    };
    if headers.contains_key(header::ORIGIN)
        || headers.contains_key(header::COOKIE)
        || !super::browser_session::static_bearer_login_available(&state)
    {
        return denied();
    }
    let Some(expected) = state.bearer_token.as_ref() else {
        return denied();
    };
    let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(labby_auth::parse_bearer_token)
    else {
        return denied();
    };
    if !labby_auth::tokens_equal(&token, expected.as_ref()) {
        return denied();
    }
    let Some(sessions) = state.static_browser_session_state.as_ref() else {
        return denied();
    };
    match sessions.start_setup_handoff(&origin) {
        Ok(token) => response(
            StatusCode::OK,
            json!({"token":token,"expires_in_seconds":60,"origin":origin}),
        ),
        Err(_) => response(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"message":"Local setup handoff is temporarily unavailable."}),
        ),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Redeem {
    token: String,
}

pub(super) async fn redeem(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    request: Result<Json<Redeem>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Some(origin) = local_origin(peer.map(|Extension(ConnectInfo(peer))| peer), &headers) else {
        return denied();
    };
    if headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        != Some(origin.as_str())
        || headers.contains_key(header::AUTHORIZATION)
        || !super::browser_session::static_bearer_login_available(&state)
        || headers
            .get("sec-fetch-site")
            .is_some_and(|value| value != "same-origin")
    {
        return denied();
    }
    let Ok(Json(request)) = request else {
        return denied();
    };
    match labby_auth::static_session::has_other_browser_session(
        &headers,
        state.project_session_state.as_deref(),
        state.oauth_state.as_deref(),
    )
    .await
    {
        Ok(false) => {}
        _ => return denied(),
    }
    let Some(sessions) = state.static_browser_session_state.as_ref() else {
        return denied();
    };
    let session = match sessions.redeem_setup_handoff(&request.token, &origin) {
        Ok(Some(session)) => session,
        _ => return denied(),
    };
    let mut result = response(StatusCode::OK, json!({"ok":true}));
    labby_auth::session::append_set_cookie(&mut result, &sessions.set_cookie(&session.session_id));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn composed_router_declares_handoff_and_refuses_missing_socket() {
        use axum::{body::Body, http::Request};
        use tower::ServiceExt;
        let state = AppState::new().with_static_browser_session_state(
            labby_auth::static_session::StaticBrowserSessionState::new(false),
        );
        let app = crate::api::router::build_router(
            state,
            Some("operator-secret".into()),
            None,
            None,
            &[],
        );
        for path in ["/auth/setup-handoff/start", "/auth/setup-handoff/redeem"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(path)
                        .header(header::HOST, "127.0.0.1:8765")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(r#"{"token":"untrusted"}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
    }

    #[tokio::test]
    async fn authenticated_local_start_redeem_and_replay_are_distinct() {
        let state = AppState::new()
            .with_bearer_token(Some(std::sync::Arc::from("operator-secret")))
            .with_static_browser_session_state(
                labby_auth::static_session::StaticBrowserSessionState::new(false),
            );
        let socket: SocketAddr = "127.0.0.1:50000".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "127.0.0.1:8765".parse().unwrap());
        assert_eq!(
            start(
                State(state.clone()),
                Some(Extension(ConnectInfo(socket))),
                headers.clone()
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        headers.insert(
            header::AUTHORIZATION,
            "Bearer operator-secret".parse().unwrap(),
        );
        assert_eq!(
            start(State(state.clone()), None, headers.clone())
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        let started = start(
            State(state.clone()),
            Some(Extension(ConnectInfo(socket))),
            headers.clone(),
        )
        .await;
        assert_eq!(started.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(started.into_body(), 1024)
            .await
            .unwrap();
        let started: Value = serde_json::from_slice(&bytes).unwrap();
        let token = started["token"].as_str().unwrap();
        assert_ne!(token, "operator-secret");
        headers.remove(header::AUTHORIZATION);
        headers.insert(header::ORIGIN, "http://127.0.0.1:9999".parse().unwrap());
        assert_eq!(
            redeem(
                State(state.clone()),
                Some(Extension(ConnectInfo(socket))),
                headers.clone(),
                Ok(Json(Redeem {
                    token: token.into()
                }))
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        headers.insert(header::ORIGIN, "http://127.0.0.1:8765".parse().unwrap());
        let response = redeem(
            State(state.clone()),
            Some(Extension(ConnectInfo(socket))),
            headers.clone(),
            Ok(Json(Redeem {
                token: token.into(),
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
        assert!(!cookie.contains(token));
        assert_eq!(
            redeem(
                State(state),
                Some(Extension(ConnectInfo(socket))),
                headers,
                Ok(Json(Redeem {
                    token: token.into()
                }))
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn loopback_socket_ip_host_and_no_forwarding_are_required() {
        let peer: SocketAddr = "127.0.0.1:55000".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "127.0.0.1:8765".parse().unwrap());
        assert_eq!(
            local_origin(Some(peer), &headers).as_deref(),
            Some("http://127.0.0.1:8765")
        );
        assert!(local_origin(None, &headers).is_none());
        assert!(local_origin(Some("10.0.0.1:50000".parse().unwrap()), &headers).is_none());
        headers.insert("x-forwarded-for", "127.0.0.1".parse().unwrap());
        assert!(local_origin(Some(peer), &headers).is_none());
        headers.remove("x-forwarded-for");
        for host in [
            "evil.test:8765",
            "localhost.evil.test",
            "127.0.0.1@evil.test",
            "127.0.0.1:8765/path",
        ] {
            headers.insert(header::HOST, host.parse().unwrap());
            assert!(local_origin(Some(peer), &headers).is_none());
        }
    }
}
