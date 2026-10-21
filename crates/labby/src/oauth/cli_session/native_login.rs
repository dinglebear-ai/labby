//! CLI orchestration for a server-hosted callback; no local listener is needed.
use super::*;
use crate::output::theme::CliTheme;
use labby_auth::upstream::{native::NativeAuthorization, runtime::UpstreamOauthRuntime};

const LOGIN_TIMEOUT: Duration = Duration::from_mins(5);
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// A new automatic login may migrate the default CLI document to hosted DCR,
/// but a saved explicit client binding remains an operator choice.
pub(super) fn preserve_client_selection(
    server: &Url,
    explicit: Option<UpstreamOauthRegistration>,
    previous: Option<&UpstreamOauthRegistration>,
) -> Result<Option<UpstreamOauthRegistration>> {
    if explicit.is_some() {
        return Ok(explicit);
    }
    Ok(match previous {
        Some(saved @ UpstreamOauthRegistration::Preregistered { .. }) => Some(saved.clone()),
        Some(saved @ UpstreamOauthRegistration::ClientMetadataDocument { url })
            if url != server.join(CLIENT_METADATA_PATH)?.as_str() =>
        {
            Some(saved.clone())
        }
        _ => None,
    })
}

/// Prefer the hosted handoff only for automatic or explicit dynamic registration.
/// Validate advertised extensions even when an explicit identity preserves loopback.
pub(super) fn plan(
    server: &Url,
    metadata: &rmcp::transport::auth::AuthorizationMetadata,
    selection: Option<&UpstreamOauthRegistration>,
) -> Result<Option<NativeAuthorization>> {
    let native = NativeAuthorization::discover(server.as_str(), metadata)?;
    if !matches!(selection, None | Some(UpstreamOauthRegistration::Dynamic)) {
        return Ok(None);
    }
    Ok(native.filter(|_| metadata.registration_endpoint.is_some()))
}

/// The identity is fresh and unpublished. Schedule best-effort local cleanup
/// when the caller cancels; cancellation never publishes this identity or starts
/// further remote operations. Already executing SQLite work may finish separately.
struct StagedIdentityGuard {
    sqlite: labby_auth::sqlite::SqliteStore,
    upstream: String,
    armed: bool,
}
impl Drop for StagedIdentityGuard {
    fn drop(&mut self) {
        if self.armed {
            let sqlite = self.sqlite.clone();
            let upstream = self.upstream.clone();
            tokio::spawn(async move {
                if sqlite
                    .clear_upstream_oauth_identity(&upstream, SUBJECT)
                    .await
                    .is_err()
                {
                    tracing::warn!("abandoned CLI native login cleanup deferred");
                }
            });
        }
    }
}

pub(super) async fn authorize(
    runtime: &UpstreamOauthRuntime,
    config: &UpstreamConfig,
    native: NativeAuthorization,
    theme: CliTheme,
) -> Result<()> {
    authorize_with_browser(runtime, config, native, theme, |url| async move {
        super::super::upstream_stdio::open_in_browser(&url).await
    })
    .await
}

async fn authorize_with_browser<F, Fut>(
    runtime: &UpstreamOauthRuntime,
    config: &UpstreamConfig,
    native: NativeAuthorization,
    theme: CliTheme,
    browser: F,
) -> Result<()>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let mut guard = StagedIdentityGuard {
        sqlite: runtime.sqlite.clone(),
        upstream: config.name.clone(),
        armed: true,
    };
    let result = tokio::time::timeout(LOGIN_TIMEOUT, async {
        let manager = runtime
            .managers
            .get(&config.name)
            .context("CLI OAuth manager unavailable")?
            .clone();
        let issuer = native.issuer().to_owned();
        let authorization = manager.begin_authorization(SUBJECT).await?;
        let csrf =
            super::super::upstream_stdio::authorization_state(&authorization.authorization_url)?;
        let session = native.start(&authorization.authorization_url).await?;
        let browser = browser(session.authorization_url().to_owned()).await;
        if let Err(error) = &browser {
            tracing::debug!(subsystem = "gateway_client", phase = "oauth.cli.browser.unavailable",
                error = %format!("{error:#}"), "automatic OAuth browser launch failed; manual authorization required");
        }
        present(
            session.authorization_url(),
            browser.is_ok(),
            &mut std::io::stderr().lock(),
            theme,
        )?;
        loop {
            if let Some(code) = session.poll().await? {
                manager
                    .complete_authorization_callback_with_issuer(
                        SUBJECT,
                        &code,
                        &csrf,
                        Some(&issuer),
                    )
                    .await?;
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    })
    .await;
    let result = match result {
        Ok(result) => result,
        Err(_) => Err(anyhow::anyhow!("CLI sign-in timed out after five minutes")),
    };
    // Await normal failure cleanup. The drop guard remains armed until it succeeds
    // so cancellation while cleaning up still gets a best-effort cleanup task.
    if result.is_err() {
        runtime
            .sqlite
            .clear_upstream_oauth_identity(&config.name, SUBJECT)
            .await?;
    }
    guard.armed = false;
    result
}

fn present(url: &str, opened: bool, output: &mut impl Write, theme: CliTheme) -> Result<()> {
    if opened {
        writeln!(output, "{}", theme.info("Browser opened for sign-in."))?;
    } else {
        writeln!(
            output,
            "{}",
            theme.warn("Could not open a browser automatically.")
        )?;
    }
    writeln!(
        output,
        "{}\n{}",
        theme.section("Open this URL in your browser to authorize:"),
        theme.accent(url)
    )?;
    writeln!(
        output,
        "{}",
        theme.muted("Waiting for authorization. This terminal will continue automatically.")
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_custom_client_binding_is_preserved_but_default_document_can_migrate() {
        let server = Url::parse("https://lab.example").unwrap();
        for saved in [
            UpstreamOauthRegistration::Preregistered {
                client_id: "owned-client".into(),
                client_secret_env: None,
            },
            UpstreamOauthRegistration::ClientMetadataDocument {
                url: "https://client.example/custom.json".into(),
            },
        ] {
            assert!(
                preserve_client_selection(&server, None, Some(&saved))
                    .unwrap()
                    .is_some()
            );
            assert!(matches!(
                preserve_client_selection(
                    &server,
                    Some(UpstreamOauthRegistration::Dynamic),
                    Some(&saved)
                )
                .unwrap(),
                Some(UpstreamOauthRegistration::Dynamic)
            ));
        }
        let default = UpstreamOauthRegistration::ClientMetadataDocument {
            url: server.join(CLIENT_METADATA_PATH).unwrap().into(),
        };
        assert!(
            preserve_client_selection(&server, None, Some(&default))
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn hosted_flow_completes_with_pkce_and_cleans_pending_on_failure_or_cancellation() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{body_string_contains, method, path},
        };
        for outcome in ["success", "start-failure", "poll-failure", "cancel"] {
            let server = MockServer::start().await;
            let origin = Url::parse(&server.uri()).unwrap();
            let metadata: rmcp::transport::auth::AuthorizationMetadata = serde_json::from_value(serde_json::json!({
                "issuer": server.uri(), "authorization_endpoint":format!("{}/authorize", server.uri()),
                "token_endpoint":format!("{}/token", server.uri()), "code_challenge_methods_supported":["S256"],
                "native_callback_endpoint":format!("{}/native/callback", server.uri()),
                "native_poll_endpoint_v2":format!("{}/native/poll", server.uri()),
                "native_authorization_start_media_type":"application/vnd.labby.native-oauth-start+json",
                "authorization_response_iss_parameter_supported":true
            })).unwrap();
            Mock::given(path("/.well-known/oauth-authorization-server"))
                .respond_with(ResponseTemplate::new(200).set_body_json(&metadata))
                .mount(&server)
                .await;
            Mock::given(path("/authorize")).and(method("GET"))
                .respond_with(if outcome == "start-failure" { ResponseTemplate::new(400) } else {
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({"authorization_url":"https://provider.example/authorize", "poll_token":"private-native-token"}))
                }).expect(1).mount(&server).await;
            Mock::given(path("/native/poll"))
                .and(method("POST"))
                .and(body_string_contains("private-native-token"))
                .respond_with(if outcome == "poll-failure" {
                    ResponseTemplate::new(400)
                } else {
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!({"code":"native-code"}))
                })
                .mount(&server)
                .await;
            Mock::given(path("/token")).and(method("POST"))
                .and(body_string_contains("code=native-code"))
                .and(body_string_contains("code_verifier="))
                .and(body_string_contains("native%2Fcallback"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"access_token":"native-access", "token_type":"Bearer", "expires_in":3600})))
                .expect(u64::from(outcome == "success")).mount(&server).await;
            let temp = tempfile::tempdir().unwrap();
            let key = base64::engine::general_purpose::STANDARD.encode([7_u8; 32]);
            let mut config = upstream(
                &origin,
                &UpstreamOauthRegistration::Preregistered {
                    client_id: "fixture-client".into(),
                    client_secret_env: None,
                },
            )
            .unwrap();
            config.name = "operator-server-native-test".into();
            let native = NativeAuthorization::discover(origin.as_str(), &metadata)
                .unwrap()
                .unwrap();
            let runtime = build_upstream_oauth_runtime_with_redirect(
                std::slice::from_ref(&config),
                &auth_config(temp.path()),
                Some(&key),
                native.callback_uri().into(),
            )
            .await
            .unwrap()
            .unwrap();
            let theme = CliTheme::from_context(
                crate::output::OutputFormat::from_json_flag(
                    false,
                    crate::output::theme::ColorPolicy::Plain,
                    crate::output::theme::RenderEnv::stderr(),
                )
                .render_context(),
            );
            if outcome == "cancel" {
                let (started, reached_browser) = tokio::sync::oneshot::channel();
                let mut flow = Box::pin(authorize_with_browser(
                    &runtime,
                    &config,
                    native,
                    theme,
                    move |_| {
                        let _ = started.send(());
                        std::future::pending::<Result<()>>()
                    },
                ));
                let early_result = tokio::select! {
                    result = &mut flow => Some(result),
                    result = tokio::time::timeout(Duration::from_secs(5), reached_browser) => {
                        result.expect("browser stage reached promptly").expect("browser stage signaled");
                        None
                    }
                };
                drop(flow);
                assert!(
                    early_result.is_none(),
                    "flow ended before cancellation: {early_result:?}"
                );
            } else {
                let result =
                    authorize_with_browser(&runtime, &config, native, theme, |url| async move {
                        assert_eq!(url, "https://provider.example/authorize");
                        Err(anyhow::anyhow!("fixture browser unavailable"))
                    })
                    .await;
                assert_eq!(
                    result.is_ok(),
                    outcome == "success",
                    "{outcome}: {result:?}"
                );
            }
            let requests = server.received_requests().await.unwrap();
            let start = requests
                .iter()
                .find(|req| req.url.path() == "/authorize")
                .expect("authorization started");
            let csrf = start
                .url
                .query_pairs()
                .find_map(|(key, value)| (key == "state").then(|| value.into_owned()))
                .unwrap();
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if runtime
                        .sqlite
                        .find_upstream_oauth_state_owner(&csrf, 0)
                        .await
                        .unwrap()
                        .is_none()
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("pending state cleaned");
            if outcome != "success" {
                assert!(
                    runtime
                        .sqlite
                        .find_upstream_oauth_credentials(&config.name, SUBJECT)
                        .await
                        .unwrap()
                        .is_none()
                );
            }
        }
    }

    #[test]
    fn hosted_instructions_show_provider_url_without_loopback_or_secret_guidance() {
        use crate::output::theme::{ColorPolicy, RenderContext, RenderEnv};
        for (policy, tty, colored) in [
            (ColorPolicy::Color, false, true),
            (ColorPolicy::Plain, true, false),
            (ColorPolicy::Auto, false, false),
            (ColorPolicy::Auto, true, true),
        ] {
            let theme = CliTheme::from_context(RenderContext::from_policy(
                policy,
                RenderEnv {
                    stream_is_tty: tty,
                    no_color: false,
                    term: Some("xterm-256color".into()),
                    colorterm: Some("truecolor".into()),
                    lang: Some("en_US.UTF-8".into()),
                    lab_symbols: None,
                    columns: Some(100),
                    ci: false,
                },
            ));
            for opened in [true, false] {
                let mut output = Vec::new();
                present(
                    "https://provider.example/authorize?state=public",
                    opened,
                    &mut output,
                    theme,
                )
                .unwrap();
                let output = String::from_utf8(output).unwrap();
                assert!(output.contains("https://provider.example/authorize?state=public"));
                assert!(output.contains("continue automatically"));
                assert!(!output.contains("ssh -L"));
                assert!(!output.contains("127.0.0.1"));
                assert!(!output.contains("poll_token"));
                assert_eq!(output.contains("\x1b["), colored);
            }
        }
    }
}
