//! Render non-secret reverse-proxy configuration for a public Labby origin.
//!
//! This is intentionally separate from the ephemeral stdio MCP proxy. It does
//! not mutate host proxy software or TLS certificates; it produces validated,
//! copy/paste-ready configuration and verification commands.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::dispatch::error::ToolError;

const DEFAULT_BACKEND_URL: &str = "http://127.0.0.1:8765";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicProxyFormat {
    Caddy,
    Nginx,
    Traefik,
    #[default]
    All,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PublicProxyRenderRequest {
    pub public_url: String,
    #[serde(default)]
    pub backend_url: Option<String>,
    #[serde(default)]
    pub format: PublicProxyFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicProxyRenderOutcome {
    pub public_origin: String,
    pub backend_origin: String,
    pub oauth_callback_url: String,
    pub mcp_url: String,
    pub recommended: String,
    pub configs: BTreeMap<String, String>,
    pub verification: Vec<String>,
}

pub fn render(request: PublicProxyRenderRequest) -> Result<PublicProxyRenderOutcome, ToolError> {
    let public = validate_origin(&request.public_url, true, "public_url")?;
    let backend = validate_backend_origin(
        request
            .backend_url
            .as_deref()
            .unwrap_or(DEFAULT_BACKEND_URL),
    )?;
    let public_origin = origin_string(&public);
    let backend_origin = origin_string(&backend);
    let host = public
        .host_str()
        .ok_or_else(|| invalid("public_url", "public URL must include a hostname"))?;
    let site = public
        .port()
        .map_or_else(|| host.to_string(), |port| format!("{host}:{port}"));

    let mut configs = BTreeMap::new();
    if matches!(
        request.format,
        PublicProxyFormat::Caddy | PublicProxyFormat::All
    ) {
        configs.insert("caddy".into(), render_caddy(&site, &backend_origin));
    }
    if matches!(
        request.format,
        PublicProxyFormat::Nginx | PublicProxyFormat::All
    ) {
        configs.insert("nginx".into(), render_nginx(host, &backend_origin));
    }
    if matches!(
        request.format,
        PublicProxyFormat::Traefik | PublicProxyFormat::All
    ) {
        configs.insert("traefik".into(), render_traefik(host, &backend_origin));
    }

    Ok(PublicProxyRenderOutcome {
        oauth_callback_url: format!("{public_origin}/auth/google/callback"),
        mcp_url: format!("{public_origin}/mcp"),
        public_origin: public_origin.clone(),
        backend_origin,
        recommended: "caddy".into(),
        configs,
        verification: vec![
            format!("curl --fail-with-body {public_origin}/health"),
            format!("curl -i {public_origin}/mcp"),
            format!("curl -fsS {public_origin}/.well-known/oauth-protected-resource || true"),
        ],
    })
}

fn validate_origin(raw: &str, require_https: bool, param: &str) -> Result<url::Url, ToolError> {
    let url =
        url::Url::parse(raw.trim()).map_err(|_| invalid(param, "must be a valid absolute URL"))?;
    let valid_scheme = if require_https {
        url.scheme() == "https"
    } else {
        matches!(url.scheme(), "http" | "https")
    };
    if !valid_scheme {
        return Err(invalid(
            param,
            if require_https {
                "public reverse-proxy URL must use HTTPS"
            } else {
                "backend URL must use HTTP or HTTPS"
            },
        ));
    }
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(invalid(
            param,
            "must be an origin only: scheme + hostname + optional port, with no credentials, path, query, or fragment",
        ));
    }
    Ok(url)
}

pub(crate) fn validate_backend_origin(raw: &str) -> Result<url::Url, ToolError> {
    validate_origin(raw, false, "backend_url")
}

pub(crate) fn origin_string(url: &url::Url) -> String {
    url.origin().ascii_serialization()
}

fn render_caddy(site: &str, backend: &str) -> String {
    format!(
        "{site} {{
    reverse_proxy {backend} {{
        # Disable response buffering so MCP streams arrive immediately.
        flush_interval -1
    }}
}}
"
    )
}

fn render_nginx(host: &str, backend: &str) -> String {
    format!(
        "# Put the map block in nginx's http context.
map $http_upgrade $connection_upgrade {{
    default upgrade;
    '' close;
}}

server {{
    listen 443 ssl http2;
    server_name {host};

    # Use your existing certificate automation or set ssl_certificate / ssl_certificate_key here.

    location / {{
        proxy_pass {backend};
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Host $host;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $connection_upgrade;
        proxy_buffering off;
        proxy_request_buffering off;
        proxy_read_timeout 3600s;
        proxy_send_timeout 3600s;
    }}
}}
"
    )
}

fn render_traefik(host: &str, backend: &str) -> String {
    format!(
        "http:
  routers:
    labby:
      rule: 'Host(\"{host}\")'
      entryPoints:
        - websecure
      tls: {{}}
      service: labby
  services:
    labby:
      loadBalancer:
        servers:
          - url: \"{backend}\"
        passHostHeader: true
"
    )
}

fn invalid(param: &str, message: &str) -> ToolError {
    ToolError::InvalidParam {
        message: message.to_string(),
        param: param.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_formats_preserve_streaming_and_forward_to_the_validated_backend() {
        let output = render(PublicProxyRenderRequest {
            public_url: "https://labby.example.com".into(),
            backend_url: None,
            format: PublicProxyFormat::All,
        })
        .unwrap();

        assert_eq!(output.public_origin, "https://labby.example.com");
        assert_eq!(output.backend_origin, DEFAULT_BACKEND_URL);
        assert_eq!(
            output.oauth_callback_url,
            "https://labby.example.com/auth/google/callback"
        );
        assert_eq!(output.mcp_url, "https://labby.example.com/mcp");
        assert_eq!(output.recommended, "caddy");
        assert_eq!(output.configs.len(), 3);
        assert!(output.configs["caddy"].contains("flush_interval -1"));
        assert!(output.configs["nginx"].contains("proxy_buffering off"));
        assert!(output.configs["nginx"].contains("proxy_request_buffering off"));
        assert!(output.configs["traefik"].contains("Host(\"labby.example.com\")"));
        assert!(
            output
                .configs
                .values()
                .all(|config| config.contains(DEFAULT_BACKEND_URL))
        );
    }

    #[test]
    fn public_proxy_rejects_insecure_or_non_origin_public_urls() {
        for invalid_url in [
            "http://labby.example.com",
            "https://user@labby.example.com",
            "https://labby.example.com/path",
            "https://labby.example.com?query=1",
            "https://labby.example.com/#fragment",
        ] {
            assert!(
                render(PublicProxyRenderRequest {
                    public_url: invalid_url.into(),
                    backend_url: None,
                    format: PublicProxyFormat::Caddy,
                })
                .is_err(),
                "accepted {invalid_url}"
            );
        }
    }

    #[test]
    fn backend_may_be_http_but_never_contains_credentials_or_paths() {
        assert!(
            render(PublicProxyRenderRequest {
                public_url: "https://labby.example.com".into(),
                backend_url: Some("http://127.0.0.1:9999".into()),
                format: PublicProxyFormat::Caddy,
            })
            .is_ok()
        );
        assert!(
            render(PublicProxyRenderRequest {
                public_url: "https://labby.example.com".into(),
                backend_url: Some("http://token@127.0.0.1:9999/private".into()),
                format: PublicProxyFormat::Caddy,
            })
            .is_err()
        );
    }
}
