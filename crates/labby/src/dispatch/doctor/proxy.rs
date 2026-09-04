use std::time::Duration;

use crate::dispatch::error::ToolError;

use super::params::ProxyCheckParams;
use super::types::{Finding, Report, Severity};

const DOCTOR_RESPONSE_MAX_BYTES: usize = 64 * 1024;

pub async fn check_proxy(params: ProxyCheckParams<'_>) -> Result<Report, ToolError> {
    // See api/state.rs::build_protected_mcp_http_client for why this call is
    // needed under "rustls-no-provider" -- idempotent, safe to ignore Err.
    drop(rustls::crypto::ring::default_provider().install_default());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to build proxy doctor client: {error}"),
        })?;

    // Resolved-address SSRF guard. `params.rs` already rejected private
    // literals and private-suffix names statically; this closes the case it
    // structurally cannot see — a public-looking name that resolves into
    // private space (split-horizon DNS, or an attacker-controlled record).
    // Only the two URLs that are actually fetched are checked; `backend_url`
    // is never requested, it is only a needle searched for in response bodies.
    for (param, url) in [("app_url", params.app_url), ("mcp_url", params.mcp_url)] {
        reject_private_resolution(param, url).await?;
    }

    let mut findings = Vec::new();
    findings.push(check_app_health(&client, params.app_url).await);
    findings.push(check_resource_metadata(&client, &params).await);
    findings.push(check_route_challenge(&client, params.mcp_url, params.route).await);
    findings.push(check_wrong_path_404(&client, params.mcp_url, params.route).await);
    if let Some(backend_url) = params.backend_url {
        findings.push(check_backend_leak(&client, params.mcp_url, params.route, backend_url).await);
    }
    Ok(Report { findings })
}

/// Resolve `url`'s host and reject if any resolved address is private.
///
/// Deliberately fails **open on resolution failure**: a name that does not
/// resolve is not an SSRF vector, and turning "DNS is down" into a hard
/// `invalid_param` would break the diagnostic's ability to report an
/// unreachable proxy as a normal finding. Only a *successful* resolution into
/// blocked space is an error. Every returned address is checked, not just the
/// first, so a mixed public/private record set cannot slip through.
async fn reject_private_resolution(param: &str, url: &str) -> Result<(), ToolError> {
    let Ok(parsed) = url::Url::parse(url) else {
        return Ok(());
    };
    // Only names need resolving. IP literals are deliberately skipped here
    // because `parse_proxy_check` already rejected private ones statically,
    // and both production call sites (`dispatch.rs` `proxy.check`, early-return
    // and main arm) go through it — a `ProxyCheckParams` cannot reach this
    // function unvalidated. The in-crate tests construct it directly against
    // `127.0.0.1` wiremock servers, which is why re-checking literals here
    // would break them. If a third production construction site is ever added,
    // it must call `parse_proxy_check` or re-check literals itself.
    let Some(url::Host::Domain(host)) = parsed.host() else {
        return Ok(());
    };
    let port = parsed.port_or_known_default().unwrap_or(443);
    let Ok(addrs) = tokio::net::lookup_host((host, port)).await else {
        return Ok(());
    };
    let redacted = labby_primitives::ssrf::redact_url(url);
    for addr in addrs {
        labby_primitives::ssrf::check_ip_not_private(addr.ip(), &redacted).map_err(|error| {
            ToolError::InvalidParam {
                message: format!(
                    "{param} resolves into private address space, which would make this probe an SSRF vector: {error}"
                ),
                param: param.to_string(),
            }
        })?;
    }
    Ok(())
}

async fn check_app_health(client: &reqwest::Client, app_url: &str) -> Finding {
    let url = join_url(app_url, "/health");
    match client.get(url).send().await {
        Ok(response) if response.status().is_success() => Finding {
            service: "doctor".to_string(),
            check: "proxy:app-health".to_string(),
            severity: Severity::Ok,
            message: "Lab app health endpoint is reachable through the proxy".to_string(),
        },
        Ok(response) => Finding {
            service: "doctor".to_string(),
            check: "proxy:app-health".to_string(),
            severity: Severity::Fail,
            message: format!("Lab app health returned HTTP {}", response.status()),
        },
        Err(error) => Finding {
            service: "doctor".to_string(),
            check: "proxy:app-health".to_string(),
            severity: Severity::Fail,
            message: request_error_message(
                "Lab app health endpoint is not reachable through the proxy",
                &error,
            ),
        },
    }
}

async fn check_resource_metadata(
    client: &reqwest::Client,
    params: &ProxyCheckParams<'_>,
) -> Finding {
    let expected_resource = join_url(params.mcp_url, params.route);
    let expected_issuer = params.app_url.trim_end_matches('/');
    let url = join_url(
        params.mcp_url,
        &format!("/.well-known/oauth-protected-resource{}", params.route),
    );
    match client.get(url).send().await {
        Ok(response) if response.status().is_success() => {
            match labby_runtime::response_body::read_response_body_capped(
                response,
                DOCTOR_RESPONSE_MAX_BYTES,
            )
            .await
            .and_then(|bytes| {
                serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|error| {
                    labby_runtime::response_body::CappedResponseBodyError::Decode(error.to_string())
                })
            }) {
                Ok(json) => {
                    let resource = json.get("resource").and_then(serde_json::Value::as_str);
                    let has_expected_issuer = json
                        .get("authorization_servers")
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(|servers| {
                            servers
                                .iter()
                                .filter_map(serde_json::Value::as_str)
                                .any(|server| server.trim_end_matches('/') == expected_issuer)
                        });
                    if resource == Some(expected_resource.as_str()) && has_expected_issuer {
                        Finding {
                            service: "doctor".to_string(),
                            check: "proxy:resource-metadata".to_string(),
                            severity: Severity::Ok,
                            message: "Protected resource metadata matches the public route and Lab issuer"
                                .to_string(),
                        }
                    } else {
                        Finding {
                            service: "doctor".to_string(),
                            check: "proxy:resource-metadata".to_string(),
                            severity: Severity::Fail,
                            message: "Protected resource metadata does not match the public route and Lab issuer"
                                .to_string(),
                        }
                    }
                }
                Err(error) => Finding {
                    service: "doctor".to_string(),
                    check: "proxy:resource-metadata".to_string(),
                    severity: Severity::Fail,
                    message: format!(
                        "Protected resource metadata response was not valid JSON: {error}"
                    ),
                },
            }
        }
        Ok(response) => Finding {
            service: "doctor".to_string(),
            check: "proxy:resource-metadata".to_string(),
            severity: Severity::Fail,
            message: format!(
                "Protected resource metadata returned HTTP {}",
                response.status()
            ),
        },
        Err(error) => Finding {
            service: "doctor".to_string(),
            check: "proxy:resource-metadata".to_string(),
            severity: Severity::Fail,
            message: request_error_message(
                "Protected resource metadata is not reachable through the proxy",
                &error,
            ),
        },
    }
}

async fn check_route_challenge(client: &reqwest::Client, mcp_url: &str, route: &str) -> Finding {
    let url = join_url(mcp_url, route);
    let expected_metadata = join_url(
        mcp_url,
        &format!("/.well-known/oauth-protected-resource{route}"),
    );
    match client.get(url).send().await {
        Ok(response)
            if response.status() == reqwest::StatusCode::UNAUTHORIZED
                && response
                    .headers()
                    .get(reqwest::header::WWW_AUTHENTICATE)
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|value| {
                        value.starts_with("Bearer ")
                            && value.contains("resource_metadata=")
                            && value.contains(&expected_metadata)
                    }) =>
        {
            Finding {
                service: "doctor".to_string(),
                check: "proxy:oauth-challenge".to_string(),
                severity: Severity::Ok,
                message: "Protected route returns an OAuth bearer challenge without credentials"
                    .to_string(),
            }
        }
        Ok(response) => Finding {
            service: "doctor".to_string(),
            check: "proxy:oauth-challenge".to_string(),
            severity: Severity::Fail,
            message: format!(
                "Protected route returned HTTP {} instead of a bearer challenge",
                response.status()
            ),
        },
        Err(error) => Finding {
            service: "doctor".to_string(),
            check: "proxy:oauth-challenge".to_string(),
            severity: Severity::Fail,
            message: request_error_message(
                "Protected route is not reachable through the proxy",
                &error,
            ),
        },
    }
}

/// Probe a path that cannot match any protected route.
///
/// A correctly configured proxy must return 404, not a backend error or a
/// leaked backend target. The "wrong path" is constructed by appending
/// `/__probe_wrong_path_lab_doctor__` to the MCP URL root, which is
/// sufficiently distinct from any operator-configured route.
async fn check_wrong_path_404(client: &reqwest::Client, mcp_url: &str, _route: &str) -> Finding {
    let url = join_url(mcp_url, "/__probe_wrong_path_lab_doctor__");
    match client.get(url).send().await {
        Ok(response) if response.status() == reqwest::StatusCode::NOT_FOUND => Finding {
            service: "doctor".to_string(),
            check: "proxy:wrong-path-404".to_string(),
            severity: Severity::Ok,
            message: "Unrecognised path returns 404 as expected".to_string(),
        },
        Ok(response) if response.status().is_server_error() => Finding {
            service: "doctor".to_string(),
            check: "proxy:wrong-path-404".to_string(),
            severity: Severity::Fail,
            message: format!(
                "Unrecognised path returned HTTP {} — backend may be leaking errors through the proxy",
                response.status()
            ),
        },
        Ok(response) => Finding {
            service: "doctor".to_string(),
            check: "proxy:wrong-path-404".to_string(),
            severity: Severity::Warn,
            message: format!(
                "Unrecognised path returned HTTP {} instead of 404",
                response.status()
            ),
        },
        Err(error) => Finding {
            service: "doctor".to_string(),
            check: "proxy:wrong-path-404".to_string(),
            severity: Severity::Fail,
            message: request_error_message(
                "Could not reach the MCP gateway for wrong-path probe",
                &error,
            ),
        },
    }
}

/// Verify the backend target URL does not appear in public error responses.
///
/// Constructs a non-matching request and checks that the response body does not
/// contain the private backend origin, which would indicate the proxy or Lab is
/// leaking internal addresses to the public internet.
async fn check_backend_leak(
    client: &reqwest::Client,
    mcp_url: &str,
    route: &str,
    backend_url: &str,
) -> Finding {
    let url = join_url(mcp_url, route);
    let backend_origin = backend_url.trim_end_matches('/');
    match client.get(url).send().await {
        Ok(response) => {
            // Read the body to check for backend URL leakage; limit to 64 KiB.
            let body = match labby_runtime::response_body::read_response_body_capped(
                response,
                DOCTOR_RESPONSE_MAX_BYTES,
            )
            .await
            {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(_) => String::new(),
            };
            if body.contains(backend_origin) {
                Finding {
                    service: "doctor".to_string(),
                    check: "proxy:backend-leak".to_string(),
                    severity: Severity::Fail,
                    message: "Response body contains the private backend origin — redact it before exposing through the proxy".to_string(),
                }
            } else {
                Finding {
                    service: "doctor".to_string(),
                    check: "proxy:backend-leak".to_string(),
                    severity: Severity::Ok,
                    message: "Response body does not expose the private backend origin".to_string(),
                }
            }
        }
        Err(error) => Finding {
            service: "doctor".to_string(),
            check: "proxy:backend-leak".to_string(),
            severity: Severity::Fail,
            message: request_error_message(
                "Could not reach the protected route for backend-leak probe",
                &error,
            ),
        },
    }
}

fn request_error_message(context: &str, error: &reqwest::Error) -> String {
    let reason = if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connection failed"
    } else if error.is_decode() {
        "invalid response body"
    } else if error.is_request() {
        "request failed"
    } else {
        "unexpected request error"
    };
    format!("{context}: {reason}")
}

fn join_url(base: &str, path: &str) -> String {
    format!("{}{}", base.trim_end_matches('/'), path)
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn proxy_check_reports_expected_public_flow() {
        let server = MockServer::start().await;
        let server_uri = server.uri();
        mount_app_health(&server).await;
        mount_matching_metadata(&server, &server_uri).await;
        mount_bearer_challenge(&server, &server_uri).await;
        mount_wrong_path_404(&server).await;

        let report = check_proxy(ProxyCheckParams {
            app_url: &server_uri,
            mcp_url: &server_uri,
            route: "/telemetry",
            backend_url: None,
        })
        .await
        .unwrap();

        assert!(
            report
                .findings
                .iter()
                .all(|finding| { matches!(finding.severity, Severity::Ok) })
        );
    }

    #[tokio::test]
    async fn proxy_check_fails_mismatched_metadata() {
        let server = MockServer::start().await;
        let server_uri = server.uri();
        mount_app_health(&server).await;
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-protected-resource/telemetry"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "resource": "https://wrong.example.com/telemetry",
                "authorization_servers":[server_uri.clone()]
            })))
            .mount(&server)
            .await;
        mount_bearer_challenge(&server, &server_uri).await;
        mount_wrong_path_404(&server).await;

        let report = check_proxy(ProxyCheckParams {
            app_url: &server_uri,
            mcp_url: &server_uri,
            route: "/telemetry",
            backend_url: None,
        })
        .await
        .unwrap();

        assert_finding_severity(&report, "proxy:resource-metadata", Severity::Fail);
    }

    #[tokio::test]
    async fn proxy_check_fails_invalid_metadata_json() {
        let server = MockServer::start().await;
        let server_uri = server.uri();
        mount_app_health(&server).await;
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-protected-resource/telemetry"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;
        mount_bearer_challenge(&server, &server_uri).await;
        mount_wrong_path_404(&server).await;

        let report = check_proxy(ProxyCheckParams {
            app_url: &server_uri,
            mcp_url: &server_uri,
            route: "/telemetry",
            backend_url: None,
        })
        .await
        .unwrap();

        assert_finding_severity(&report, "proxy:resource-metadata", Severity::Fail);
    }

    #[tokio::test]
    async fn proxy_check_fails_non_bearer_challenge() {
        let server = MockServer::start().await;
        let server_uri = server.uri();
        mount_app_health(&server).await;
        mount_matching_metadata(&server, &server_uri).await;
        Mock::given(method("GET"))
            .and(path("/telemetry"))
            .respond_with(
                ResponseTemplate::new(401)
                    .insert_header("www-authenticate", "Basic realm=\"legacy\""),
            )
            .mount(&server)
            .await;
        mount_wrong_path_404(&server).await;

        let report = check_proxy(ProxyCheckParams {
            app_url: &server_uri,
            mcp_url: &server_uri,
            route: "/telemetry",
            backend_url: None,
        })
        .await
        .unwrap();

        assert_finding_severity(&report, "proxy:oauth-challenge", Severity::Fail);
    }

    #[tokio::test]
    async fn proxy_check_fails_wrong_path_returns_server_error() {
        let server = MockServer::start().await;
        let server_uri = server.uri();
        mount_app_health(&server).await;
        mount_matching_metadata(&server, &server_uri).await;
        mount_bearer_challenge(&server, &server_uri).await;
        // Wrong-path returns 502 (backend error leaked through proxy)
        Mock::given(method("GET"))
            .and(path("/__probe_wrong_path_lab_doctor__"))
            .respond_with(ResponseTemplate::new(502))
            .mount(&server)
            .await;

        let report = check_proxy(ProxyCheckParams {
            app_url: &server_uri,
            mcp_url: &server_uri,
            route: "/telemetry",
            backend_url: None,
        })
        .await
        .unwrap();

        assert_finding_severity(&report, "proxy:wrong-path-404", Severity::Fail);
    }

    #[tokio::test]
    async fn proxy_check_detects_backend_url_leak() {
        let server = MockServer::start().await;
        let server_uri = server.uri();
        let backend_url = "http://internal-mcp-backend:3100";
        mount_app_health(&server).await;
        mount_matching_metadata(&server, &server_uri).await;
        // Protected route response leaks the backend origin in the body
        Mock::given(method("GET"))
            .and(path("/telemetry"))
            .respond_with(
                ResponseTemplate::new(401)
                    .set_body_string(format!(
                        "Bearer error, backend at {backend_url} was not reachable"
                    ))
                    .insert_header(
                        "www-authenticate",
                        format!(
                            "Bearer resource_metadata=\"{server_uri}/.well-known/oauth-protected-resource/telemetry\""
                        ),
                    ),
            )
            .mount(&server)
            .await;
        mount_wrong_path_404(&server).await;

        let report = check_proxy(ProxyCheckParams {
            app_url: &server_uri,
            mcp_url: &server_uri,
            route: "/telemetry",
            backend_url: Some(backend_url),
        })
        .await
        .unwrap();

        assert_finding_severity(&report, "proxy:backend-leak", Severity::Fail);
    }

    #[tokio::test]
    async fn proxy_check_backend_leak_ok_when_origin_absent() {
        let server = MockServer::start().await;
        let server_uri = server.uri();
        let backend_url = "http://internal-mcp-backend:3100";
        mount_app_health(&server).await;
        mount_matching_metadata(&server, &server_uri).await;
        mount_bearer_challenge(&server, &server_uri).await;
        mount_wrong_path_404(&server).await;

        let report = check_proxy(ProxyCheckParams {
            app_url: &server_uri,
            mcp_url: &server_uri,
            route: "/telemetry",
            backend_url: Some(backend_url),
        })
        .await
        .unwrap();

        assert_finding_severity(&report, "proxy:backend-leak", Severity::Ok);
    }

    async fn mount_app_health(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200))
            .mount(server)
            .await;
    }

    async fn mount_matching_metadata(server: &MockServer, server_uri: &str) {
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-protected-resource/telemetry"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "resource": format!("{server_uri}/telemetry"),
                "authorization_servers": [server_uri],
            })))
            .mount(server)
            .await;
    }

    async fn mount_bearer_challenge(server: &MockServer, server_uri: &str) {
        Mock::given(method("GET"))
            .and(path("/telemetry"))
            .respond_with(ResponseTemplate::new(401).insert_header(
                "www-authenticate",
                format!(
                    "Bearer resource_metadata=\"{server_uri}/.well-known/oauth-protected-resource/telemetry\""
                ),
            ))
            .mount(server)
            .await;
    }

    async fn mount_wrong_path_404(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/__probe_wrong_path_lab_doctor__"))
            .respond_with(ResponseTemplate::new(404))
            .mount(server)
            .await;
    }

    fn assert_finding_severity(report: &Report, check: &str, severity: Severity) {
        let finding = report
            .findings
            .iter()
            .find(|finding| finding.check == check)
            .unwrap_or_else(|| panic!("missing finding `{check}`"));
        assert!(matches!(
            (finding.severity, severity),
            (Severity::Ok, Severity::Ok)
                | (Severity::Warn, Severity::Warn)
                | (Severity::Fail, Severity::Fail)
        ));
    }
}
