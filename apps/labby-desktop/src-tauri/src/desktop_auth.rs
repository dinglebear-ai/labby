//! Native browser handoff for Control Plane sign-in.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};

const START_PATH: &str = "/auth/desktop/start";
const AUTHORIZE_PATH: &str = "/auth/desktop/authorize";
const POLL_PATH: &str = "/auth/desktop/poll";
const REDEEM_PATH: &str = "/auth/desktop/redeem";
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const MAX_FLOW_DURATION: Duration = Duration::from_secs(5 * 60);
const MAX_AUTH_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Debug, PartialEq, Eq)]
enum PollResult {
    Ready,
    RetryAfter(Duration),
}

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

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
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

async fn decode_json_bounded<T: DeserializeOwned>(
    mut response: reqwest::Response,
    context: &str,
) -> Result<T, String> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_AUTH_RESPONSE_BYTES as u64)
    {
        return Err(format!("Labby returned an oversized {context}"));
    }
    let capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or_default()
        .min(MAX_AUTH_RESPONSE_BYTES);
    let mut body = Vec::with_capacity(capacity);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("Labby returned an invalid {context}: {error}"))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_AUTH_RESPONSE_BYTES {
            return Err(format!("Labby returned an oversized {context}"));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body)
        .map_err(|error| format!("Labby returned an invalid {context}: {error}"))
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
    let response: StartResponse = decode_json_bounded(response, "sign-in response").await?;
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
) -> Result<PollResult, String> {
    let response = client
        .post(endpoint(origin, POLL_PATH)?)
        .header(reqwest::header::ORIGIN, origin)
        .json(&PollRequest { poll_token })
        .send()
        .await
        .map_err(|error| format!("could not check sign-in: {error}"))?;
    let status = response.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        // Labby emits Retry-After as delta-seconds. Missing or malformed values
        // use its conservative one-minute rate-limit recovery interval.
        let delay = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(60))
            .max(POLL_INTERVAL)
            .min(MAX_FLOW_DURATION);
        return Ok(PollResult::RetryAfter(delay));
    }
    if !matches!(
        status,
        reqwest::StatusCode::ACCEPTED | reqwest::StatusCode::OK
    ) {
        return Err(format!("Labby could not complete sign-in ({status})"));
    }
    let response: PollResponse = decode_json_bounded(response, "sign-in status").await?;
    poll_ready(status, &response, now).map(|ready| {
        if ready {
            PollResult::Ready
        } else {
            PollResult::RetryAfter(POLL_INTERVAL)
        }
    })
}

async fn poll_until_ready<F, Fut>(
    deadline: tokio::time::Instant,
    is_current: impl Fn() -> bool,
    mut poll: F,
) -> Result<bool, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<PollResult, String>>,
{
    let mut delay = Duration::ZERO;
    loop {
        if !delay.is_zero() {
            let wake = (tokio::time::Instant::now() + delay).min(deadline);
            tokio::time::sleep_until(wake).await;
        }
        if !is_current() {
            return Ok(false);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("Sign-in timed out. Try again.".to_owned());
        }
        let response = tokio::time::timeout_at(deadline, poll())
            .await
            .map_err(|_| "Sign-in timed out. Try again.".to_owned())??;
        if !is_current() {
            return Ok(false);
        }
        match response {
            PollResult::Ready => return Ok(true),
            PollResult::RetryAfter(retry_after) => delay = retry_after,
        }
    }
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
    if !poll_until_ready(
        deadline,
        || app.state::<DesktopAuthState>().is_current(generation),
        || post_poll(&client, &origin, &response.poll_token, unix_now()),
    )
    .await?
    {
        return Ok(());
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
    fn start_response_body_is_bounded_without_content_length() {
        let (origin, request) = mock_once(|_| {
            let body = "x".repeat(MAX_AUTH_RESPONSE_BYTES + 1);
            format!(
                "HTTP/1.1 201 Created\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{body}"
            )
        });
        let error = run_async(post_start(&test_client(), &origin, "challenge", "/"))
            .expect_err("oversized response must be rejected");
        assert!(error.contains("oversized sign-in response"));
        request.join().unwrap();
    }

    #[test]
    fn poll_response_body_is_bounded_without_content_length() {
        let (origin, request) = mock_once(|_| {
            let body = "x".repeat(MAX_AUTH_RESPONSE_BYTES + 1);
            format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{body}"
            )
        });
        let error = run_async(post_poll(&test_client(), &origin, "poll", 1_000))
            .expect_err("oversized response must be rejected");
        assert!(error.contains("oversized sign-in status"));
        request.join().unwrap();
    }

    #[test]
    fn default_poll_cadence_leaves_room_in_the_server_rate_budget() {
        assert!(POLL_INTERVAL >= Duration::from_secs(2));
    }

    #[test]
    fn throttled_poll_is_recoverable_instead_of_ending_sign_in() {
        let (origin, request) = mock_once(|_| {
            "HTTP/1.1 429 Too Many Requests\r\nretry-after: 2\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".into()
        });
        let result = run_async(post_poll(&test_client(), &origin, "poll", 1_000));
        request.join().unwrap();
        assert!(result.is_ok(), "429 must remain recoverable: {result:?}");
    }

    #[test]
    fn polling_retries_a_throttled_response_then_completes() {
        let (first_origin, first_request) = mock_once(|_| {
            "HTTP/1.1 429 Too Many Requests\r\nretry-after: 2\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".into()
        });
        let (second_origin, second_request) = mock_once(|_| {
            let body = r#"{"ready":true,"expires_at":2000}"#;
            format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
        });
        let calls = std::cell::Cell::new(0);
        let started = tokio::time::Instant::now();
        let result = run_async(poll_until_ready(
            started + Duration::from_secs(10),
            || true,
            || {
                let origin = if calls.get() == 0 {
                    first_origin.clone()
                } else {
                    second_origin.clone()
                };
                calls.set(calls.get() + 1);
                async move { post_poll(&test_client(), &origin, "poll", 1_000).await }
            },
        ));
        first_request.join().unwrap();
        second_request.join().unwrap();
        assert_eq!(result, Ok(true));
        assert_eq!(calls.get(), 2);
        assert!(started.elapsed() >= POLL_INTERVAL);
    }

    #[test]
    fn throttled_poll_stops_at_deadline_without_a_fast_retry() {
        let calls = std::cell::Cell::new(0);
        let started = tokio::time::Instant::now();
        let result = run_async(async {
            poll_until_ready(
                tokio::time::Instant::now() + Duration::from_millis(100),
                || true,
                || {
                    calls.set(calls.get() + 1);
                    async { Ok(PollResult::RetryAfter(Duration::from_secs(60))) }
                },
            )
            .await
        });
        assert!(result.unwrap_err().contains("timed out"));
        assert_eq!(calls.get(), 1);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn poll_request_itself_cannot_outlive_the_flow_deadline() {
        let result = run_async(poll_until_ready(
            tokio::time::Instant::now() + Duration::from_millis(30),
            || true,
            std::future::pending,
        ));
        assert!(result.unwrap_err().contains("timed out"));
    }

    #[test]
    fn throttled_poll_uses_a_bounded_nonzero_retry_delay() {
        for (header, expected) in [("0", 2), ("invalid", 60), ("18446744073709551615", 300)] {
            let (origin, request) = mock_once(move |_| {
                format!(
                    "HTTP/1.1 429 Too Many Requests\r\nretry-after: {header}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                )
            });
            let result = run_async(post_poll(&test_client(), &origin, "poll", 1_000));
            request.join().unwrap();
            assert_eq!(
                result,
                Ok(PollResult::RetryAfter(Duration::from_secs(expected)))
            );
        }
    }

    #[test]
    fn poll_protocol_handles_pending_then_ready_mock_responses() {
        for (status, ready, expected) in [(202, false, false), (200, true, true)] {
            let (origin, request) = mock_once(move |_| {
                let body = serde_json::json!({"ready": ready, "expires_at": 2_000}).to_string();
                format!(
                    "HTTP/1.1 {status} Test\r\ncontent-type: applica