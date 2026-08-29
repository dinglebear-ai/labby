use axum::{
    Json,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};

use crate::error::{AuthError, AuthErrorKind};
use crate::types::TokenResponse;

pub(super) enum TokenEndpointError {
    Auth(AuthError),
    UnsupportedGrantType(String),
}

impl TokenEndpointError {
    fn oauth_error(&self) -> &'static str {
        match self {
            Self::Auth(AuthError::InvalidGrant(_)) => "invalid_grant",
            Self::Auth(AuthError::InvalidScope(_)) => "invalid_scope",
            Self::Auth(AuthError::OauthNeedsReauth(_)) => "oauth_needs_reauth",
            Self::UnsupportedGrantType(_) => "unsupported_grant_type",
            Self::Auth(AuthError::AuthFailed(_) | AuthError::InvalidAccessToken) => {
                "invalid_client"
            }
            Self::Auth(AuthError::RateLimited { .. }) => "temporarily_unavailable",
            Self::Auth(AuthError::Validation(_)) => "invalid_request",
            Self::Auth(
                AuthError::Config(_)
                | AuthError::Storage(_)
                | AuthError::Network(_)
                | AuthError::Server(_)
                | AuthError::Decode(_)
                | AuthError::InsecurePermissions { .. },
            ) => "server_error",
        }
    }

    fn log_kind(&self) -> &'static str {
        match self {
            Self::Auth(error) => error.kind(),
            Self::UnsupportedGrantType(_) => "unsupported_grant_type",
        }
    }

    fn status(&self) -> StatusCode {
        match self {
            Self::Auth(
                AuthError::InvalidGrant(_) | AuthError::InvalidScope(_) | AuthError::Validation(_),
            )
            | Self::UnsupportedGrantType(_) => StatusCode::BAD_REQUEST,
            Self::Auth(
                AuthError::OauthNeedsReauth(_)
                | AuthError::AuthFailed(_)
                | AuthError::InvalidAccessToken,
            ) => StatusCode::UNAUTHORIZED,
            Self::Auth(AuthError::RateLimited { .. }) => StatusCode::TOO_MANY_REQUESTS,
            Self::Auth(
                AuthError::Config(_)
                | AuthError::Storage(_)
                | AuthError::Network(_)
                | AuthError::Server(_)
                | AuthError::Decode(_)
                | AuthError::InsecurePermissions { .. },
            ) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn description(&self) -> String {
        if self.oauth_error() == "invalid_client" {
            return "invalid client credentials".to_string();
        }
        match self {
            Self::Auth(error) => error.to_string(),
            Self::UnsupportedGrantType(grant_type) => {
                format!("unsupported grant_type `{grant_type}`")
            }
        }
    }

    fn retry_after_ms(&self) -> Option<u64> {
        match self {
            Self::Auth(AuthError::RateLimited { retry_after_ms, .. }) => Some(*retry_after_ms),
            _ => None,
        }
    }
}

impl IntoResponse for TokenEndpointError {
    fn into_response(self) -> Response {
        let status = self.status();
        let log_kind = self.log_kind();
        let retry_after_ms = self.retry_after_ms();
        let body = Json(labby_oauth_wire::OAuthErrorResponse {
            error: self.oauth_error().to_string(),
            error_description: self.description(),
            error_uri: None,
        });
        let mut response = (status, body).into_response();
        response.extensions_mut().insert(AuthErrorKind(log_kind));
        if let Some(retry_after_ms) = retry_after_ms
            && let Ok(value) = HeaderValue::from_str(&(retry_after_ms / 1_000).max(1).to_string())
        {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        apply_token_cache_headers(response)
    }
}

pub(super) struct TokenResponseWithCache(pub(super) Json<TokenResponse>);

impl IntoResponse for TokenResponseWithCache {
    fn into_response(self) -> Response {
        apply_token_cache_headers(self.0.into_response())
    }
}

pub(super) fn apply_token_cache_headers(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response
}
