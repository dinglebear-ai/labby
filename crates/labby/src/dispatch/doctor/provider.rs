//! Provider-aware Doctor checks shared by CLI, MCP, and HTTP.

use super::{Finding, Severity};

pub async fn live_probe(config: Option<&labby_auth::config::AuthConfig>) -> Finding {
    let Some(config) = config else {
        return Finding {
            service: "auth".into(),
            check: "auth:live-provider-probe".into(),
            severity: Severity::Fail,
            message: "kind=config_error; resolve auth configuration before probing; verify provider-specific environment variables".into(),
        };
    };
    let Some(authelia) = config.authelia.clone() else {
        return Finding {
            service: "auth".into(),
            check: "auth:live-provider-probe".into(),
            severity: Severity::Warn,
            message: "kind=unsupported_provider; live discovery/JWKS probing is currently available for Authelia".into(),
        };
    };
    let Some(public_url) = config.public_url.as_ref() else {
        return Finding {
            service: "auth".into(),
            check: "auth:live-provider-probe".into(),
            severity: Severity::Fail,
            message: "kind=config_error; LABBY_PUBLIC_URL is required for the live provider probe"
                .into(),
        };
    };
    let Ok(redirect) =
        public_url.join(labby_auth::config::AUTHELIA_CALLBACK_PATH.trim_start_matches('/'))
    else {
        return Finding {
            service: "auth".into(),
            check: "auth:live-provider-probe".into(),
            severity: Severity::Fail,
            message: "kind=config_error; public URL cannot form the provider callback".into(),
        };
    };

    match tokio::time::timeout(
        std::time::Duration::from_secs(35),
        labby_auth::authelia::AutheliaProvider::live_probe(authelia, redirect),
    )
    .await
    {
        Ok(Ok(())) => Finding {
            service: "auth".into(),
            check: "auth:live-provider-probe".into(),
            severity: Severity::Ok,
            message: "Authelia discovery and JWKS probe passed".into(),
        },
        Ok(Err(error)) => {
            tracing::warn!(
                surface = "doctor",
                phase = "auth.live_probe",
                provider = "authelia",
                kind = error.kind(),
                error = %error,
                "provider discovery/JWKS probe failed"
            );
            Finding {
                service: "auth".into(),
                check: "auth:live-provider-probe".into(),
                severity: Severity::Fail,
                message: format!(
                    "kind={}; provider discovery/JWKS probe failed; verify issuer reachability and the configured trust policy",
                    error.kind()
                ),
            }
        }
        Err(_) => Finding {
            service: "auth".into(),
            check: "auth:live-provider-probe".into(),
            severity: Severity::Fail,
            message: "kind=network_error; provider discovery/JWKS probe exceeded 35 seconds; verify issuer reachability".into(),
        },
    }
}
