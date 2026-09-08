//! Native browser handoff for Control Plane sign-in.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};

const START_PATH: &str = "/auth/desktop/start";
const AUTHORIZE_PATH: &str = "/auth/desktop/authorize";
const POLL_PATH: &str = "/auth/desktop/poll";
const REDEEM_PATH: &str = "/auth/desktop/redeem";
const POLL_INTERVAL: Duration = Duration::from_millis(750);
const MAX_FLOW_DURATION: Duration = Duration::from_secs(5 * 60);

#[derive(Default)]
pub(crate) struct DesktopAuthState(AtomicU64);

impl DesktopAuthState {
    fn begin(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst).wrapping_add(1).max(1)
    }

    fn is_current(&self, generation: u64) -> bool {
        self.0.load(Ordering::SeqCst) == generation
    }

    pub(crate) fn cancel(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Serialize)]
struct StartRequest<'a> {
    code_challenge: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    return_to: Option<&'a str>,
}

#[derive(Deserialize)]
struct StartResponse {
    authorization_url: String,
    poll_token: String,
    redeem_code: String,
    expires_at: i64,
}

#[derive(Serialize)]
struct PollRequest<'a> {
    poll_token: &'a str,
}

#[derive(Deserialize)]
struct PollResponse {
    ready: bool,
    expires_at: i64,
}

#[derive(Serialize)]
struct RedeemRequest<'a> {
    redeem_code: &'a str,
    code_verifier: &'a str,
}

pub(crate) fn login_return_to(url: &tauri::Url, control_plane_origin: &str) -> Option<String> {
    let origin = reqwest::Url::parse(control_plane_origin).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.origin().ascii_serialization() != origin.origin().ascii_serialization()
        || url.path() != "/auth/login"
    {
        return None;
    }
    url.query_pairs()
        .find_map(|(key, value)| (key == "return_to").then(|| value.into_owned()))
        .filter(|value| safe_return_to(value))
        .or_else(|| Some("/".to_owned()))
}

fn safe_return_to(value: &str) -> bool {
    value.starts_with('/')
        && !value.starts_with("//")
        && !value.contains("..")
        && !value.contains(['\\', '\0', '\r', '\n'])
}

fn pkce_pair() -> Result<(String, String), String> {
    let mut random = [0_u8; 32];
    getrandom::fill(&mut random)
        .map_err(|error| format!("could not create sign-in proof: {error}"))?;
    let verifier = URL_SAFE_NO_PAD.encode(random);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    Ok((verifier, challenge))
}

fn endpoint(origin: &str, path: &str) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(origin).map_err(|error| error.to_string())?;
    url.set_path(path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn validate_authorization_url(origin: &str, candidate: &str) -> Result<reqwest::Url, String> {
    let expected = reqwest::Url::parse(origin).map_err(|error| error.to_string())?;
    let candidate = reqwest::Url::parse(candidate)
        .map_err(|_| "Labby returned an invalid browser authorization URL".to_owned())?;
    let loopback_http =
        candidate.scheme() == "http" && candidate.host_str().is_some_and(super::is_loopback_host);
    if !(candidate.scheme() == "https" || loopback_http)
        || candidate.origin() != expected.origin()
        || candidate.path() != AUTHORIZE_PATH
        || candidate.username() != ""
        || candidate.password().is_some()
        || candidate.fragment().is_some()
    {
        return Err("Labby returned an untrusted browser authorization URL".to_owned());
    }
    let mut query = candidate.query_pairs();
    if !query
        .next()
        .is_some_and(|(key, value)| key == "state" && !value.is_empty())
        || query.next().is_some()
    {
        return Err("Labby returned an invalid browser authorization state".to_owned());
    }
    Ok(candidate)
}

fn poll_ready(
    status: reqwest::StatusCode,
    response: &PollResponse,
    now: i64,
) -> Result<bool, String> {
    if response.expires_at <= now {
        return Err("The sign-in request expired".to_owned());
    }
    match (status, response.ready) {
        (reqwest::StatusCode::ACCEPTED, false) => Ok(false),
        (reqwest::StatusCode::OK, true) => Ok(true),
        _ => Err("Labby returned an inconsistent sign-in status".to_owned()),
    }
}

async fn post_start(
    client: &reqwest::Client,
    origin: &str,
    challenge: &str,
    return_to: &str,
) -> Result<StartResponse, String> {
    let response = client
        .post(endpoint(origin, START_PATH)?)
        .header(reqwest::header::ORIGIN, origin)
        .json(&StartRequest {
            code_challenge: challenge,
            return_to: Some(return_to),
        })
        .send()
        .await
        .map_err(|error| format!("could not start sign-in: {error}"))?;
    if response.status() != reqwest::StatusCode::CREATED {
        return Err(format!("Labby rejected sign-in ({})", response.status()));
    }
    let response: StartResponse = response
        .json()
        .await
        .map_err(|error| format!("Labby returned an invalid sign-in response: {error}"))?;
    if response.poll_token.is_empty() || response.redeem_code.is_empty() {
        return Err("Labby returned an incomplete sign-in response".to_owned());
    }
    Ok(response)
}

async fn post_poll(
    client: &reqwest::Client,
    origin: &str,
    poll_token: &str,
    now: i64,
) -> Result<bool, String> {
    let response = client
        .post(endpoint(origin, POLL_PATH)?)
        .header(reqwest::header::ORIGIN, origin)
        .json(&PollRequest { poll_token })
        .send()
        .await
        .map_err(|error| format!("could not check sign-in: {error}"))?;
    let status = response.status();
    if !matches!(
        status,
        reqwest::StatusCode::ACCEPTED | reqwest::StatusCode::OK
    ) {
        return Err(format!("Labby could not complete sign-in ({status})"));
    }
    let response: PollResponse = response
        .json()
        .await
        .map_err(|error| format!("Labby returned an invalid sign-in status: {error}"))?;
    poll_ready(status, &response, now)
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .try_into()
        .unwrap_or(i64::MAX)
}

fn status_script(message: &str, failure: bool, generation: u64) -> String {
    let message = serde_json::to_string(message).unwrap_or_else(|_| "\"Sign-in failed\"".into());
    let role = if failure { "alert" } else { "status" };
    format!(
        r#"(() => {{
          window.__labbyDesktopAuthGeneration = {generation};
          let node = document.getElementById('labby-desktop-auth-status');
          if (!node) {{
            node = document.createElement('div');
            node.id = 'labby-desktop-auth-status';
            node.setAttribute('aria-live', 'polite');
            node.style.cssText = 'position:fixed;z-index:2147483647;left:50%;bottom:24px;transform:translateX(-50%);max-width:min(560px,calc(100vw - 32px));padding:12px 16px;border:1px solid rgba(127,127,127,.45);border-radius:10px;background:Canvas;color:CanvasText;box-shadow:0 8px 32px rgba(0,0,0,.25);font:14px system-ui,sans-serif';
            document.body.appendChild(node);
          }}
          node.setAttribute('role', '{role}');
          node.textContent = {message};
        }})()"#
    )
}

fn redeem_script(
    response: &StartResponse,
    verifier: &str,
    return_to: &str,
    origin: &str,
    generation: u64,
) -> String {
    let body = serde_json::to_string(&RedeemRequest {
        redeem_code: &response.redeem_code,
        code_verifier: verifier,
    })
    .expect("serializing strings cannot fail");
    let body = serde_json::to_string(&body).expect("serializing strings cannot fail");
    let return_to = serde_json::to_string(return_to).expect("serializing strings cannot fail");
    let origin = serde_json::to_string(origin).expect("serializing strings cannot fail");
    format!(
        r#"(async () => {{
          if (window.__labbyDesktopAuthGeneration !== {generation} || location.origin !== {origin}) return;
          try {{
            const response = await fetch('{REDEEM_PATH}', {{
              method: 'POST', credentials: 'include',
              headers: {{'content-type': 'application/json'}}, body: {body}
            }});
            if (response.status !== 204) throw new Error(`session exchange failed (${{response.status}})`);
            if (window.__labbyDesktopAuthGeneration === {generation}) location.replace({return_to});
          }} catch (error) {{
            if (window.__labbyDesktopAuthGeneration !== {generation}) return;
            const node = document.getElementById('labby-desktop-auth-status');
            if (node) {{ node.setAttribute('role', 'alert'); node.textContent = `Sign-in failed: ${{error}}. Try again.`; }}
          }}
        }})()"#
    )
}

fn show_status(app: &AppHandle, generation: u64, message: &str, failure: bool) {
    if !app.state::<DesktopAuthState>().is_current(generation) {
        return;
    }
    if let Some(window) = app.get_webview_window(super::CONTROL_PLANE_WINDOW)
        && let Err(error) = window.eval(status_script(message, failure, generation))
    {
        super::warn("failed to render desktop sign-in status", error);
    }
}

async fn run(app: AppHandle, generation: u64, return_to: String) -> Result<(), String> {
    let origin = super::configured_origin(&app)?;
    let (verifier, challenge) = pkce_pair()?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| error.to_string())?;
    let response = post_start(&client, &origin, &challenge, &return_to).await?;
    let authorization_url = validate_authorization_url(&origin, &response.authorization_url)?;
    if response.expires_at <= unix_now() {
        return Err("The sign-in request expired before it could open".to_owned());
    }
    if !app.state::<DesktopAuthState>().is_current(generation) {
        return Ok(());
    }
    open::that(authorization_url.as_str())
        .map_err(|error| format!("could not open the system browser: {error}"))?;
    show_status(
        &app,
        generation,
        "Finish signing in in your browser. Labby will return here automatically.",
        false,
    );

    let expires_in =
        u64::try_from(response.expires_at.saturating_sub(unix_now())).unwrap_or_default();
    let deadline =
        tokio::time::Instant::now() + Duration::from_secs(expires_in).min(MAX_FLOW_DURATION);
    loop {
        if !app.state::<DesktopAuthState>().is_current(generation) {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("Sign-in timed out. Try again.".to_owned());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
        if post_poll(&client, &origin, &response.poll_token, unix_now()).await? {
            break;
        }
    }

    if !app.state::<DesktopAuthState>().is_current(generation) {
        return Ok(());
    }
    if super::configured_origin(&app)? != origin {
        return Ok(());
    }
    let window = app
        .get_webview_window(super::CONTROL_PLANE_WINDOW)
        .ok_or_else(|| "Control Plane window is unavailable".to_owned())?;
    let current_url = window
        .url()
        .map_err(|error| format!("could not verify the Control Plane page: {error}"))?;
    if current_url.origin().ascii_serialization() != origin {
        return Ok(());
    }
    show_status(
        &app,
        generation,
        "Completing sign-in in the Control Plane…",
        false,
    );
    window
        .eval(redeem_script(
            &response, &verifier, &return_to, &origin, generation,
        ))
        .map_err(|error| format!("could not create the signed-in session: {error}"))
}

pub(crate) fn begin(app: AppHandle, return_to: String) {
    let generation = app.state::<DesktopAuthState>().begin();
    show_status(
        &app,
        generation,
        "Opening secure sign-in in your browser…",
        false,
    );
    tauri::async_runtime::spawn(async move {
        if let Err(error) = run(app.clone(), generation, return_to).await {
            show_status(&app, generation, &format!("Sign-in failed: {error}"), true);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn mock_once(
        response: impl FnOnce(&str) -> String + Send + 'static,
    ) -> (String, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let response_origin = origin.clone();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let count = stream.read(&mut buffer).unwrap_or_default();
                if count == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..count]);
                let text = String::from_utf8_lossy(&bytes);
                let Some((headers, body)) = text.split_once("\r\n\r\n") else {
                    continue;
                };
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|value| value.parse::<usize>().ok())
                    })
                    .unwrap_or_default();
                if body.len() >= length {
                    break;
                }
            }
            stream
                .write_all(response(&response_origin).as_bytes())
                .unwrap();
            String::from_utf8(bytes).unwrap()
        });
        (origin, handle)
    }

    fn test_client() -> reqwest::Client {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    }

    fn run_async<T>(future: impl Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    fn execute_browser_script(script: &str, prelude: &str) -> serde_json::Value {
        let harness = format!(
            r#"
const nodes = new Map();
const body = {{ appendChild(node) {{ nodes.set(node.id, node); }} }};
global.document = {{
  body,
  getElementById(id) {{ return nodes.get(id) || null; }},
  createElement() {{
    return {{ id: '', attrs: {{}}, style: {{}}, textContent: '', setAttribute(k, v) {{ this.attrs[k] = v; }} }};
  }}
}};
const result = {{ fetchCalls: 0, navigations: [] }};
global.window = global;
global.location = {{
  origin: 'https://labby.example.com',
  replace(value) {{ result.navigations.push(value); }}
}};
global.fetch = async () => {{ result.fetchCalls += 1; return {{status: 204}}; }};
{prelude}
{script}
setTimeout(() => {{
  const node = nodes.get('labby-desktop-auth-status');
  result.generation = window.__labbyDesktopAuthGeneration;
  result.status = node ? {{role: node.attrs.role, live: node.attrs['aria-live'], text: node.textContent}} : null;
  process.stdout.write(JSON.stringify(result));
}}, 30);
"#
        );
        let output = std::process::Command::new("node")
            .args(["-e", &harness])
            .output()
            .expect("Node.js is required to build and test the desktop web assets");
        assert!(
            output.status.success(),
            "script harness failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn redeem_test_script(generation: u64) -> String {
        redeem_script(
            &StartResponse {
                authorization_url: "https://labby.example.com/auth/desktop/authorize?state=s"
                    .into(),
                poll_token: "poll".into(),
                redeem_code: "redeem".into(),
                expires_at: i64::MAX,
            },
            "verifier",
            "/settings/",
            "https://labby.example.com",
            generation,
        )
    }

    #[test]
    fn pkce_is_fresh_and_matches_s256() {
        let (first, challenge) = pkce_pair().unwrap();
        let (second, _) = pkce_pair().unwrap();
        assert_ne!(first, second);
        assert_eq!(first.len(), 43);
        assert_eq!(
            challenge,
            URL_SAFE_NO_PAD.encode(Sha256::digest(first.as_bytes()))
        );
    }

    #[test]
    fn only_exact_same_origin_login_navigation_starts_native_auth() {
        let origin = "https://labby.example.com";
        assert_eq!(
            login_return_to(
                &tauri::Url::parse("https://labby.example.com/auth/login?return_to=%2Fsettings%2F")
                    .unwrap(),
                origin,
            ),
            Some("/settings/".to_owned())
        );
        for rejected in [
            "https://labby.example.com/auth/login/",
            "https://attacker.invalid/auth/login",
            "https://labby.example.com.evil/auth/login",
        ] {
            assert_eq!(
                login_return_to(&tauri::Url::parse(rejected).unwrap(), origin),
                None
            );
        }
    }

    #[test]
    fn unsafe_return_target_falls_back_to_root() {
        let url = tauri::Url::parse(
            "https://labby.example.com/auth/login?return_to=https%3A%2F%2Fattacker.invalid",
        )
        .unwrap();
        assert_eq!(
            login_return_to(&url, "https://labby.example.com"),
            Some("/".into())
        );
    }

    #[test]
    fn authorization_url_is_server_owned_and_strictly_confined() {
        let origin = "https://labby.example.com";
        assert!(
            validate_authorization_url(
                origin,
                "https://labby.example.com/auth/desktop/authorize?state=opaque"
            )
            .is_ok()
        );
        for rejected in [
            "https://accounts.google.com/o/oauth2/auth?state=opaque",
            "https://labby.example.com.evil/auth/desktop/authorize?state=opaque",
            "https://labby.example.com/auth/desktop/start?state=opaque",
            "https://labby.example.com/auth/desktop/authorize",
            "https://labby.example.com/auth/desktop/authorize?state=",
            "https://labby.example.com/auth/desktop/authorize?state=one&state=two",
            "https://labby.example.com/auth/desktop/authorize?state=one&next=evil",
            "javascript:alert(1)",
            "https://user@labby.example.com/auth/desktop/authorize?state=opaque",
        ] {
            assert!(
                validate_authorization_url(origin, rejected).is_err(),
                "accepted {rejected}"
            );
        }
    }

    #[test]
    fn poll_protocol_accepts_only_matching_status_and_body() {
        let pending = PollResponse {
            ready: false,
            expires_at: 2_000,
        };
        let ready = PollResponse {
            ready: true,
            expires_at: 2_000,
        };
        assert_eq!(
            poll_ready(reqwest::StatusCode::ACCEPTED, &pending, 1_000),
            Ok(false)
        );
        assert_eq!(poll_ready(reqwest::StatusCode::OK, &ready, 1_000), Ok(true));
        assert!(poll_ready(reqwest::StatusCode::OK, &pending, 1_000).is_err());
        assert!(poll_ready(reqwest::StatusCode::ACCEPTED, &ready, 1_000).is_err());
        assert!(poll_ready(reqwest::StatusCode::ACCEPTED, &pending, 2_000).is_err());
    }

    #[test]
    fn start_protocol_posts_origin_and_does_not_follow_redirects() {
        let (origin, request) = mock_once(|origin| {
            let body = serde_json::json!({
                "authorization_url": format!("{origin}{AUTHORIZE_PATH}?state=opaque"),
                "poll_token": "poll",
                "redeem_code": "redeem",
                "expires_at": i64::MAX,
            })
            .to_string();
            format!(
                "HTTP/1.1 201 Created\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
        });
        let response = run_async(post_start(
            &test_client(),
            &origin,
            "challenge",
            "/settings/",
        ));
        assert!(response.is_ok());
        let request = request.join().unwrap();
        assert!(request.starts_with("POST /auth/desktop/start HTTP/1.1\r\n"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains(&format!("origin: {origin}").to_ascii_lowercase())
        );
        assert!(request.contains("\"code_challenge\":\"challenge\""));
        assert!(request.contains("\"return_to\":\"/settings/\""));

        let (origin, request) = mock_once(|_| {
            "HTTP/1.1 302 Found\r\nlocation: https://attacker.invalid/\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_owned()
        });
        let error = match run_async(post_start(&test_client(), &origin, "challenge", "/")) {
            Ok(_) => panic!("redirect response must not be accepted"),
            Err(error) => error,
        };
        assert!(error.contains("302"));
        request.join().unwrap();
    }

    #[test]
    fn poll_protocol_handles_pending_then_ready_mock_responses() {
        for (status, ready, expected) in [(202, false, false), (200, true, true)] {
            let (origin, request) = mock_once(move |_| {
                let body = serde_json::json!({"ready": ready, "expires_at": 2_000}).to_string();
                format!(
                    "HTTP/1.1 {status} Test\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                )
            });
            assert_eq!(
                run_async(post_poll(&test_client(), &origin, "poll-secret", 1_000)).unwrap(),
                expected
            );
            let request = request.join().unwrap();
            assert!(request.starts_with("POST /auth/desktop/poll HTTP/1.1\r\n"));
            assert!(request.contains("\"poll_token\":\"poll-secret\""));
        }
    }

    #[test]
    fn mock_poll_rejects_inconsistent_or_expired_response() {
        for (status, ready, expires_at) in
            [(200, false, 2_000), (202, true, 2_000), (202, false, 999)]
        {
            let (origin, request) = mock_once(move |_| {
                let body =
                    serde_json::json!({"ready": ready, "expires_at": expires_at}).to_string();
                format!(
                    "HTTP/1.1 {status} Test\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                )
            });
            assert!(run_async(post_poll(&test_client(), &origin, "poll", 1_000)).is_err());
            request.join().unwrap();
        }
    }

    #[test]
    fn a_new_flow_invalidates_the_previous_generation() {
        let state = DesktopAuthState::default();
        let first = state.begin();
        assert!(state.is_current(first));
        let second = state.begin();
        assert!(!state.is_current(first));
        assert!(state.is_current(second));
    }

    #[test]
    fn redeem_is_same_origin_cookie_fetch_and_requires_no_content() {
        let response = StartResponse {
            authorization_url: "https://labby.example.com/auth/desktop/authorize?state=s".into(),
            poll_token: "poll".into(),
            redeem_code: "code\"</script>".into(),
            expires_at: i64::MAX,
        };
        let script = redeem_script(
            &response,
            "verifier",
            "/settings/",
            "https://labby.example.com",
            7,
        );
        assert!(script.contains("fetch('/auth/desktop/redeem'"));
        assert!(script.contains("credentials: 'include'"));
        assert!(script.contains("response.status !== 204"));
        assert!(script.contains("location.origin !== \"https://labby.example.com\""));
        assert!(script.contains("location.replace(\"/settings/\")"));
        assert!(!script.contains("code\"</script>"));
    }

    #[test]
    fn progress_overlay_keeps_the_hosted_document_and_is_accessible() {
        let script = status_script("Continue in browser", false, 3);
        assert!(script.contains("aria-live"));
        assert!(script.contains("role', 'status"));
        assert!(!script.contains("document.body.innerHTML"));
    }

    #[test]
    fn executed_status_script_creates_accessible_live_region() {
        let result = execute_browser_script(&status_script("Continue in browser", false, 7), "");
        assert_eq!(result["generation"], 7);
        assert_eq!(result["status"]["role"], "status");
        assert_eq!(result["status"]["live"], "polite");
        assert_eq!(result["status"]["text"], "Continue in browser");
    }

    #[test]
    fn executed_redeem_on_204_navigates_to_internal_return() {
        let result = execute_browser_script(
            &format!(
                "{};\n{}",
                status_script("Completing", false, 7),
                redeem_test_script(7)
            ),
            "",
        );
        assert_eq!(result["fetchCalls"], 1);
        assert_eq!(result["navigations"], serde_json::json!(["/settings/"]));
    }

    #[test]
    fn executed_redeem_failures_show_error_without_navigation() {
        for prelude in [
            "global.fetch = async () => { result.fetchCalls += 1; return {status: 500}; };",
            "global.fetch = async () => { result.fetchCalls += 1; throw new Error('offline'); };",
        ] {
            let result = execute_browser_script(
                &format!(
                    "{};\n{}",
                    status_script("Completing", false, 7),
                    redeem_test_script(7)
                ),
                prelude,
            );
            assert_eq!(result["fetchCalls"], 1);
            assert_eq!(result["navigations"], serde_json::json!([]));
            assert_eq!(result["status"]["role"], "alert");
            assert!(
                result["status"]["text"]
                    .as_str()
                    .unwrap()
                    .contains("Sign-in failed")
            );
        }
    }

    #[test]
    fn stale_generation_and_wrong_origin_never_execute_fetch() {
        for prelude in [
            "window.__labbyDesktopAuthGeneration = 8;",
            "window.__labbyDesktopAuthGeneration = 7; location.origin = 'https://attacker.invalid';",
        ] {
            let result = execute_browser_script(&redeem_test_script(7), prelude);
            assert_eq!(result["fetchCalls"], 0);
            assert_eq!(result["navigations"], serde_json::json!([]));
        }
    }

    #[test]
    fn replacement_attempt_suppresses_stale_completion_after_fetch() {
        let result = execute_browser_script(
            &redeem_test_script(7),
            r#"
window.__labbyDesktopAuthGeneration = 7;
global.fetch = () => {
  result.fetchCalls += 1;
  return new Promise(resolve => {
    setTimeout(() => { window.__labbyDesktopAuthGeneration = 8; }, 0);
    setTimeout(() => resolve({status: 204}), 5);
  });
};
"#,
        );
        assert_eq!(result["fetchCalls"], 1);
        assert_eq!(result["generation"], 8);
        assert_eq!(result["navigations"], serde_json::json!([]));
    }
}
