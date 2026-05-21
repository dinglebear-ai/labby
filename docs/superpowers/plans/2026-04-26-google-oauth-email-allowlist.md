# Google OAuth Email Allowlist Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an optional email allowlist to the Google OAuth browser-login flow so that only explicitly permitted email addresses can create sessions, while leaving the behavior unchanged when no allowlist is configured.

**Architecture:** A new `LAB_AUTH_ALLOWED_EMAILS` env var (comma-separated) is read into `AuthConfig.allowed_emails: Vec<String>` (pre-normalized to lowercase at startup). The `check_email_allowlist` helper in `authorize.rs` enforces the list in the browser-login path (returning 401 on denial). The OAuth-client callback path uses an inline RFC 6749-compliant error redirect (`error=access_denied`) rather than the shared helper, because registered clients must receive their error via redirect, not a bare JSON response. An empty list means "no restriction". The `email_verified` claim from Google's id_token is enforced before the email comparison to prevent the unverified-email bypass attack.

**Tech Stack:** Rust, thiserror (`AuthError`), wiremock (test HTTP mocking), cargo-nextest

---

## File Map

| File | Change |
|------|--------|
| `crates/lab-auth/src/google.rs` | Add `email_verified: Option<bool>` to `GoogleIdTokenClaims` and `GoogleExchange`; thread through both construction sites |
| `crates/lab-auth/src/config.rs` | Add `allowed_emails: Vec<String>` field; parse + pre-normalize from `LAB_AUTH_ALLOWED_EMAILS`; add startup warning when set-but-empty |
| `crates/lab-auth/src/authorize.rs` | Add `check_email_allowlist` helper (with `email_verified` guard and fingerprinted logging); enforce in browser-login branch with `?`; enforce in OAuth-client branch with an inline error redirect |
| `docs/OAUTH.md` | Add `LAB_AUTH_ALLOWED_EMAILS` to the environment variable table |
| `.env.example` | Add `LAB_AUTH_ALLOWED_EMAILS=` entry with comment |

---

## Task 1: Add `email_verified` to the Google provider types

**Files:**
- Modify: `crates/lab-auth/src/google.rs`

`GoogleIdTokenClaims` (line 74) does not include `email_verified`. Without it, an attacker who creates a Google account with any email address (unverified) can bypass the allowlist because the id_token signature is valid even when `email_verified: false`. This task fixes that before any allowlist enforcement is wired in.

- [ ] **Step 1: Write the failing test**

In `crates/lab-auth/src/google.rs`, add to the existing `#[cfg(test)]` block:

```rust
#[tokio::test]
async fn google_exchange_exposes_email_verified_claim() {
    // This test will fail until email_verified is added to the structs.
    // The mock token in signed_test_id_token_verified(false) has email_verified: false.
    // After this task, GoogleExchange.email_verified should carry that value through.
    let exchange = GoogleExchange {
        subject: "sub".to_string(),
        email: Some("user@example.com".to_string()),
        email_verified: Some(false),
        access_token: "tok".to_string(),
        refresh_token: None,
        expires_in: None,
        id_token: "id".to_string(),
    };
    assert_eq!(exchange.email_verified, Some(false));
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p lab-auth google_exchange_exposes_email_verified 2>&1 | tail -10
```

Expected: compilation error — `email_verified` field does not exist on `GoogleExchange`.

- [ ] **Step 3: Add `email_verified` to `GoogleIdTokenClaims` and `GoogleExchange`**

In `crates/lab-auth/src/google.rs`:

Change `GoogleIdTokenClaims` (line 73–79) from:
```rust
#[derive(Debug, Deserialize)]
struct GoogleIdTokenClaims {
    iss: String,
    sub: String,
    #[serde(default)]
    email: Option<String>,
}
```

To:
```rust
#[derive(Debug, Deserialize)]
struct GoogleIdTokenClaims {
    iss: String,
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: Option<bool>,
}
```

Change `GoogleExchange` (line 53–61) from:
```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoogleExchange {
    pub subject: String,
    pub email: Option<String>,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    pub id_token: String,
}
```

To:
```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoogleExchange {
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: Option<bool>,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    pub id_token: String,
}
```

Update the first `Ok(GoogleExchange {...})` construction site (around line 351):
```rust
Ok(GoogleExchange {
    subject: claims.sub,
    email: claims.email,
    email_verified: claims.email_verified,   // ← add
    access_token: payload.access_token,
    refresh_token: payload.refresh_token,
    expires_in: payload.expires_in,
    id_token: payload.id_token,
})
```

Update the second `Ok(GoogleExchange {...})` construction site in `refresh` (around line 394):
```rust
Ok(GoogleExchange {
    subject: claims.sub,
    email: claims.email,
    email_verified: claims.email_verified,   // ← add
    access_token: payload.access_token,
    refresh_token: payload.refresh_token,
    expires_in: payload.expires_in,
    id_token: payload.id_token,
})
```

- [ ] **Step 4: Update the test helper `signed_test_id_token` to include `email_verified: true`**

The existing `signed_test_id_token()` in the `#[cfg(test)]` block of `authorize.rs` does not include `email_verified`. Update its claims to include it so existing tests keep working and the claim is testable:

```rust
fn signed_test_id_token() -> String {
    let claims = json!({
        "iss": "https://accounts.google.com",
        "aud": "client-id",
        "sub": "google-subject-123",
        "email": "user@example.com",
        "email_verified": true,          // ← add this line
        "iat": now_unix() as usize,
        "exp": (now_unix() + 3600) as usize,
    });
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("test-kid".to_string());
    encode(&header, &claims, &test_encoding_key()).unwrap()
}
```

Add a companion helper for unverified email tests (add right after `signed_test_id_token`):
```rust
fn signed_test_id_token_unverified() -> String {
    let claims = json!({
        "iss": "https://accounts.google.com",
        "aud": "client-id",
        "sub": "google-subject-123",
        "email": "user@example.com",
        "email_verified": false,
        "iat": now_unix() as usize,
        "exp": (now_unix() + 3600) as usize,
    });
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("test-kid".to_string());
    encode(&header, &claims, &test_encoding_key()).unwrap()
}
```

- [ ] **Step 5: Run the full test suite to verify no regressions**

```bash
cargo test -p lab-auth 2>&1 | tail -30
```

Expected: all existing tests pass. The new `google_exchange_exposes_email_verified` test passes.

- [ ] **Step 6: Commit**

```bash
git add crates/lab-auth/src/google.rs crates/lab-auth/src/authorize.rs
git commit -m "feat(auth): add email_verified claim to GoogleIdTokenClaims and GoogleExchange"
```

---

## Task 2: Add `allowed_emails` to `AuthConfig` with pre-normalization

**Files:**
- Modify: `crates/lab-auth/src/config.rs`

Entries are normalized to lowercase and trimmed at config load so comparisons at runtime are simple string equality. This also prevents operator misconfiguration (mixed case, trailing whitespace) from silently locking out valid users.

- [ ] **Step 1: Write the failing tests**

Add to the existing `#[cfg(test)]` block in `crates/lab-auth/src/config.rs`:

```rust
#[test]
fn allowed_emails_defaults_to_empty_vec() {
    let cfg = AuthConfig::from_sources(fake_env_with_many([
        ("LAB_AUTH_MODE", "oauth"),
        ("LAB_PUBLIC_URL", "https://lab.example.com"),
        ("LAB_GOOGLE_CLIENT_ID", "id"),
        ("LAB_GOOGLE_CLIENT_SECRET", "secret"),
    ]))
    .unwrap();
    assert!(cfg.allowed_emails.is_empty());
}

#[test]
fn allowed_emails_normalizes_case_and_trims_whitespace() {
    let cfg = AuthConfig::from_sources(fake_env_with_many([
        ("LAB_AUTH_MODE", "oauth"),
        ("LAB_PUBLIC_URL", "https://lab.example.com"),
        ("LAB_GOOGLE_CLIENT_ID", "id"),
        ("LAB_GOOGLE_CLIENT_SECRET", "secret"),
        ("LAB_AUTH_ALLOWED_EMAILS", "Alice@Example.com , BOB@EXAMPLE.COM "),
    ]))
    .unwrap();
    assert_eq!(
        cfg.allowed_emails,
        vec!["alice@example.com".to_string(), "bob@example.com".to_string()]
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p lab-auth allowed_emails 2>&1 | tail -10
```

Expected: compilation error — `allowed_emails` field does not exist yet.

- [ ] **Step 3: Add `allowed_emails` to `AuthConfig`**

In `crates/lab-auth/src/config.rs`, add the field to the `AuthConfig` struct after `allowed_client_redirect_uris`:

```rust
pub struct AuthConfig {
    pub mode: AuthMode,
    pub public_url: Option<Url>,
    pub sqlite_path: PathBuf,
    pub key_path: PathBuf,
    pub bootstrap_secret: Option<String>,
    pub allowed_client_redirect_uris: Vec<String>,
    pub allowed_emails: Vec<String>,
    pub google: GoogleConfig,
    pub access_token_ttl: Duration,
    pub refresh_token_ttl: Duration,
    pub auth_code_ttl: Duration,
    pub register_requests_per_minute: u32,
    pub authorize_requests_per_minute: u32,
    pub max_pending_oauth_states: usize,
}
```

Update `Default::default()` for `AuthConfig`:
```rust
allowed_client_redirect_uris: Vec::new(),
allowed_emails: Vec::new(),
google: GoogleConfig::default(),
```

Update `AuthConfig::from_sources`. Replace the existing `allowed_client_redirect_uris` line with:
```rust
allowed_client_redirect_uris: read_csv(&vars, "LAB_AUTH_ALLOWED_REDIRECT_URIS")
    .unwrap_or_default(),
allowed_emails: read_csv(&vars, "LAB_AUTH_ALLOWED_EMAILS")
    .unwrap_or_default()
    .into_iter()
    .map(|e| e.to_ascii_lowercase())
    .collect(),
```

Note: `read_csv` already trims whitespace from each entry (the `map(str::trim)` at line 238). The `.to_ascii_lowercase()` is added here, not inside `read_csv`, to keep that helper generic.

- [ ] **Step 4: Add a startup warning when the allowlist env var is set-but-empty**

In `AuthConfig::from_sources`, after the `allowed_emails` assignment, add a validation note. The warning cannot be emitted here (no tracing context), so handle it in `validate`. Add to the `validate` method:

```rust
fn validate(&self) -> Result<(), AuthError> {
    if !self.google.callback_path.starts_with('/') {
        return Err(AuthError::Config(format!(
            "LAB_GOOGLE_CALLBACK_PATH must start with `/`, got `{}`",
            self.google.callback_path
        )));
    }

    if matches!(self.mode, AuthMode::OAuth) {
        if self.public_url.is_none() {
            return Err(AuthError::Config(
                "LAB_PUBLIC_URL is required when LAB_AUTH_MODE=oauth".to_string(),
            ));
        }
        if self.google.client_id.is_empty() {
            return Err(AuthError::Config(
                "LAB_GOOGLE_CLIENT_ID is required when LAB_AUTH_MODE=oauth".to_string(),
            ));
        }
        if self.google.client_secret.is_empty() {
            return Err(AuthError::Config(
                "LAB_GOOGLE_CLIENT_SECRET is required when LAB_AUTH_MODE=oauth".to_string(),
            ));
        }
    }

    Ok(())
}
```

The warning for set-but-empty must be emitted at startup when tracing is available. In `crates/lab-auth/src/state.rs`, in `AuthState::new`, add after the `info!("lab-auth state initialized")` block:

```rust
if state_config.allowed_emails.is_empty() {
    if std::env::var("LAB_AUTH_ALLOWED_EMAILS")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
    {
        // env var was set but parsed to empty (e.g., only commas or whitespace)
        warn!(
            env_var = "LAB_AUTH_ALLOWED_EMAILS",
            "allowed_emails env var is set but resolved to an empty list — \
             all Google accounts are permitted. Check for typos or whitespace-only values."
        );
    }
}
```

Wait — `AuthState::new` takes `config: AuthConfig` (moved), and `Arc::new(config)` happens inside. Read the actual construction order in `state.rs` to position this correctly. The config is moved into `Arc::new(config)` at the end. Place the warning **before** the move:

```rust
pub async fn new(config: AuthConfig) -> Result<Self, AuthError> {
    // ... existing validation ...

    // Warn if LAB_AUTH_ALLOWED_EMAILS is set in env but resolved empty.
    // This happens when the value is whitespace-only or comma-only.
    // An empty list means ALL Google accounts are permitted — which may surprise
    // an operator who thought they enabled the allowlist.
    if config.allowed_emails.is_empty()
        && std::env::var("LAB_AUTH_ALLOWED_EMAILS")
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
    {
        warn!(
            env_var = "LAB_AUTH_ALLOWED_EMAILS",
            "allowed_emails env var is set but resolved to an empty list — \
             all Google accounts are permitted. Check for typos or whitespace-only values."
        );
    }

    // ... rest of AuthState::new ...
}
```

- [ ] **Step 5: Run the config tests**

```bash
cargo test -p lab-auth allowed_emails 2>&1 | tail -20
```

Expected: both `allowed_emails_defaults_to_empty_vec` and `allowed_emails_normalizes_case_and_trims_whitespace` PASS.

- [ ] **Step 6: Run the full lab-auth test suite**

```bash
cargo test -p lab-auth 2>&1 | tail -30
```

Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/lab-auth/src/config.rs crates/lab-auth/src/state.rs
git commit -m "feat(auth): add LAB_AUTH_ALLOWED_EMAILS config field with normalization"
```

---

## Task 3: Add the `check_email_allowlist` helper with unit tests

**Files:**
- Modify: `crates/lab-auth/src/authorize.rs`

The helper is pure (no I/O), so its logic is fully covered by unit tests. Integration tests are only needed to verify the HTTP status code and redirect behavior at the call sites.

- [ ] **Step 1: Write the failing unit tests**

In `authorize.rs`, inside the `#[cfg(test)]` block, add unit tests for the helper directly. These use no wiremock:

```rust
mod allowlist_tests {
    use super::check_email_allowlist;

    #[test]
    fn empty_allowlist_permits_any_email() {
        assert!(check_email_allowlist(Some("anyone@example.com"), Some(true), &[]).is_ok());
    }

    #[test]
    fn empty_allowlist_permits_even_unverified_email() {
        // When no allowlist is configured, email_verified is not enforced.
        assert!(check_email_allowlist(Some("anyone@example.com"), Some(false), &[]).is_ok());
    }

    #[test]
    fn matching_verified_email_is_permitted() {
        let list = vec!["alice@example.com".to_string()];
        assert!(check_email_allowlist(Some("alice@example.com"), Some(true), &list).is_ok());
    }

    #[test]
    fn matching_email_is_case_insensitive() {
        // Allowlist is pre-normalized to lowercase at config load.
        // Incoming email from Google may have any case.
        let list = vec!["alice@example.com".to_string()];
        assert!(check_email_allowlist(Some("Alice@Example.com"), Some(true), &list).is_ok());
    }

    #[test]
    fn non_matching_email_is_rejected() {
        let list = vec!["alice@example.com".to_string()];
        assert!(check_email_allowlist(Some("eve@example.com"), Some(true), &list).is_err());
    }

    #[test]
    fn unverified_email_is_rejected_even_when_in_allowlist() {
        let list = vec!["alice@example.com".to_string()];
        assert!(check_email_allowlist(Some("alice@example.com"), Some(false), &list).is_err());
    }

    #[test]
    fn missing_email_verified_claim_is_rejected_when_allowlist_is_set() {
        let list = vec!["alice@example.com".to_string()];
        assert!(check_email_allowlist(Some("alice@example.com"), None, &list).is_err());
    }

    #[test]
    fn none_email_is_rejected_when_allowlist_is_set() {
        let list = vec!["alice@example.com".to_string()];
        assert!(check_email_allowlist(None, Some(true), &list).is_err());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p lab-auth allowlist_tests 2>&1 | tail -10
```

Expected: compilation error — `check_email_allowlist` function does not exist yet.

- [ ] **Step 3: Add the `check_email_allowlist` helper**

In `crates/lab-auth/src/authorize.rs`, above the `browser_login` function, add:

```rust
fn check_email_allowlist(
    email: Option<&str>,
    email_verified: Option<bool>,
    allowed_emails: &[String],
) -> Result<(), AuthError> {
    if allowed_emails.is_empty() {
        return Ok(());
    }
    if email_verified != Some(true) {
        error!("browser login rejected: google did not return a verified email address");
        return Err(AuthError::AuthFailed(
            "google did not return a verified email address".to_string(),
        ));
    }
    match email {
        Some(e) if allowed_emails.iter().any(|a| a.eq_ignore_ascii_case(e)) => Ok(()),
        Some(e) => {
            warn!(
                email_id = %fingerprint(e),
                "browser login rejected: email not in allowed list"
            );
            Err(AuthError::AuthFailed(
                "google account is not permitted to access this gateway".to_string(),
            ))
        }
        None => {
            error!("browser login rejected: google did not return an email address");
            Err(AuthError::AuthFailed(
                "google did not return an email address".to_string(),
            ))
        }
    }
}
```

Notes on this implementation:
- `error!` (not `warn!`) for missing/unverified email — these indicate a server-side misconfiguration (missing `email` scope or failed OIDC setup), not an expected user rejection
- `warn!` for "email not in allowlist" — this is expected behavior, not a bug
- `email_id = %fingerprint(e)` instead of raw email — matches the convention used for `subject_id`, `oauth_state_id`, `auth_code_id` everywhere else in this file; prevents PII in logs
- `fingerprint` is already imported via `use crate::util::fingerprint` at the top of the file; verify this import is present and add it if missing

- [ ] **Step 4: Run the unit tests to verify they pass**

```bash
cargo test -p lab-auth allowlist_tests 2>&1 | tail -20
```

Expected: all 8 unit tests PASS.

- [ ] **Step 5: Run the full test suite**

```bash
cargo test -p lab-auth 2>&1 | tail -30
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/lab-auth/src/authorize.rs
git commit -m "feat(auth): add check_email_allowlist helper with email_verified enforcement"
```

---

## Task 4: Enforce the allowlist in the browser-login callback branch

**Files:**
- Modify: `crates/lab-auth/src/authorize.rs`

The browser-login branch returns 401 JSON on rejection — correct because there is no registered OAuth client redirect to honor.

- [ ] **Step 1: Write the failing integration test**

In the `#[cfg(test)]` block, add one integration test covering the rejection path:

```rust
#[tokio::test]
async fn browser_login_callback_rejects_email_not_in_allowlist() {
    let mut config = test_auth_config();
    // "allowed@example.com" is permitted; mock token returns "user@example.com" → denied
    config.allowed_emails = vec!["allowed@example.com".to_string()];
    let base_state = test_auth_state_with_config(config).await;
    base_state
        .store
        .register_client(RegisteredClient {
            client_id: "client".to_string(),
            redirect_uris: vec!["http://127.0.0.1:7777/callback".to_string()],
            created_at: now_unix(),
        })
        .await
        .unwrap();

    let server = Box::leak(Box::new(MockServer::start().await));
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "google-access-token",
            "refresh_token": "refresh-token",
            "expires_in": 3600,
            "id_token": signed_test_id_token(), // email="user@example.com", email_verified=true
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/certs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks()))
        .mount(server)
        .await;

    let google = GoogleProvider::new(
        "client-id".to_string(),
        "client-secret".to_string(),
        Url::parse("https://lab.example.com/auth/google/callback").unwrap(),
    )
    .unwrap()
    .with_endpoints(
        server.uri().parse::<Url>().unwrap(),
        server.uri().parse::<Url>().unwrap().join("/token").unwrap(),
    )
    .with_jwks_endpoint(server.uri().parse::<Url>().unwrap().join("/certs").unwrap());

    let state = AuthState::for_tests(
        (*base_state.config).clone(),
        base_state.store.clone(),
        (*base_state.signing_keys).clone(),
        google,
    );
    let app = router(state.clone());

    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/login?return_to=%2F")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let location = Url::parse(
        login.headers().get(header::LOCATION).unwrap().to_str().unwrap(),
    )
    .unwrap();
    let upstream_state = location
        .query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
        .unwrap();

    let callback = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/auth/google/callback?state={upstream_state}&code=upstream-code"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(callback.status(), StatusCode::UNAUTHORIZED);
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p lab-auth browser_login_callback_rejects_email_not_in_allowlist 2>&1 | tail -10
```

Expected: test compiles but FAILS — callback returns 303 SEE_OTHER (session is created) instead of 401.

- [ ] **Step 3: Add the allowlist check to the browser-login branch**

In the `callback` handler (around line 226), the browser-login branch looks like:

```rust
if let Some(login) = state.store.take_browser_login_state(&query.state).await? {
    let google = state
        .google
        .exchange_code(&query.code, &login.provider_code_verifier)
        .await?;
    let session = create_browser_session(&state, google.subject, google.email).await?;
```

Change it to:

```rust
if let Some(login) = state.store.take_browser_login_state(&query.state).await? {
    let google = state
        .google
        .exchange_code(&query.code, &login.provider_code_verifier)
        .await?;
    check_email_allowlist(
        google.email.as_deref(),
        google.email_verified,
        &state.config.allowed_emails,
    )?;
    let session = create_browser_session(&state, google.subject, google.email).await?;
```

The `?` operator is correct here: `AuthError::AuthFailed` → 401 JSON response. There is no OAuth client redirect to honor in this branch.

- [ ] **Step 4: Run the integration test to verify it passes**

```bash
cargo test -p lab-auth browser_login_callback_rejects_email_not_in_allowlist 2>&1 | tail -10
```

Expected: PASS — callback returns 401.

- [ ] **Step 5: Run the full test suite**

```bash
cargo test -p lab-auth 2>&1 | tail -30
```

Expected: all tests pass (including the existing `browser_login_callback_sets_session_cookie_and_redirects_home`, which uses an empty allowlist and continues to pass through).

- [ ] **Step 6: Commit**

```bash
git add crates/lab-auth/src/authorize.rs
git commit -m "feat(auth): enforce email allowlist in browser-login callback branch"
```

---

## Task 5: Enforce the allowlist in the OAuth-client callback branch (RFC 6749 compliant)

**Files:**
- Modify: `crates/lab-auth/src/authorize.rs`

The OAuth-client branch must NOT use `check_email_allowlist(...)?`. Propagating `AuthError::AuthFailed` via `?` would return JSON 401 to the browser, breaking registered MCP clients (Claude.ai, IDE extensions) that expect to receive `error=access_denied` via redirect to their registered `redirect_uri`. RFC 6749 §4.1.2.1 requires the error redirect.

The `request` row has already been consumed from the store via `take_authorization_request`, so `request.redirect_uri` and `request.client_state` are in scope when the check fires.

- [ ] **Step 1: Write the failing integration test**

Add to the `#[cfg(test)]` block:

```rust
#[tokio::test]
async fn oauth_client_callback_redirects_with_access_denied_when_email_not_in_allowlist() {
    let mut config = test_auth_config();
    config.allowed_emails = vec!["allowed@example.com".to_string()];
    let base_state = test_auth_state_with_config(config).await;
    base_state
        .store
        .register_client(RegisteredClient {
            client_id: "client".to_string(),
            redirect_uris: vec!["http://127.0.0.1:7777/callback".to_string()],
            created_at: now_unix(),
        })
        .await
        .unwrap();
    // Pre-insert an authorization request (OAuth-client flow, not browser-login flow)
    base_state
        .store
        .insert_authorization_request(AuthorizationRequestRow {
            state: "good-state".to_string(),
            client_id: "client".to_string(),
            redirect_uri: "http://127.0.0.1:7777/callback".to_string(),
            client_state: "client-abc".to_string(),
            scope: "lab".to_string(),
            provider_code_verifier: "provider-verifier".to_string(),
            code_challenge: "challenge".to_string(),
            code_challenge_method: "S256".to_string(),
            created_at: now_unix(),
            expires_at: now_unix() + 300,
        })
        .await
        .unwrap();

    let server = Box::leak(Box::new(MockServer::start().await));
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "google-access-token",
            "refresh_token": "refresh-token",
            "expires_in": 3600,
            "id_token": signed_test_id_token(), // email="user@example.com", not in allowlist
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/certs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks()))
        .mount(server)
        .await;

    let google = GoogleProvider::new(
        "client-id".to_string(),
        "client-secret".to_string(),
        Url::parse("https://lab.example.com/auth/google/callback").unwrap(),
    )
    .unwrap()
    .with_endpoints(
        server.uri().parse::<Url>().unwrap(),
        server.uri().parse::<Url>().unwrap().join("/token").unwrap(),
    )
    .with_jwks_endpoint(server.uri().parse::<Url>().unwrap().join("/certs").unwrap());

    let state = AuthState::for_tests(
        (*base_state.config).clone(),
        base_state.store.clone(),
        (*base_state.signing_keys).clone(),
        google,
    );
    let app = router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/google/callback?state=good-state&code=upstream-code")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Must redirect (not 401) with error=access_denied and the original client state
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap();
    let redirect = Url::parse(location).unwrap();
    let params: std::collections::HashMap<_, _> = redirect.query_pairs().collect();
    assert_eq!(params.get("error").map(|v| v.as_ref()), Some("access_denied"));
    assert_eq!(params.get("state").map(|v| v.as_ref()), Some("client-abc"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p lab-auth oauth_client_callback_redirects_with_access_denied 2>&1 | tail -10
```

Expected: test compiles and FAILS — currently no allowlist check in this branch.

- [ ] **Step 3: Add the inline error redirect to the OAuth-client branch**

In the `callback` handler, find the OAuth-client branch. After `take_authorization_request` succeeds and `exchange_code` completes (around line 265–270), the code looks like:

```rust
let google = state
    .google
    .exchange_code(&query.code, &request.provider_code_verifier)
    .await?;
let subject_id = fingerprint(&google.subject);
```

Change to:

```rust
let google = state
    .google
    .exchange_code(&query.code, &request.provider_code_verifier)
    .await?;

// Allowlist check for the OAuth-client branch.
// Per RFC 6749 §4.1.2.1, errors must be sent via redirect to the client's redirect_uri,
// not returned as a direct HTTP error response. Do NOT use `?` here.
if let Err(_) = check_email_allowlist(
    google.email.as_deref(),
    google.email_verified,
    &state.config.allowed_emails,
) {
    let mut redirect_target =
        reqwest::Url::parse(&request.redirect_uri).map_err(|e| {
            AuthError::Server(format!("failed to parse registered redirect_uri: {e}"))
        })?;
    redirect_target
        .query_pairs_mut()
        .append_pair("error", "access_denied")
        .append_pair("error_description", "google account is not permitted to access this gateway")
        .append_pair("state", &request.client_state);
    warn!(
        client_id = %request.client_id,
        oauth_state_id = %oauth_state_id,
        "oauth callback: email not in allowlist, redirecting client with access_denied"
    );
    return Ok(Redirect::to(redirect_target.as_str()).into_response());
}

let subject_id = fingerprint(&google.subject);
```

Verify that `Redirect` is imported. The existing `browser_login` function uses `Redirect::to(...)`, so the import should already exist at the top of the file (`use axum::response::Redirect`).

- [ ] **Step 4: Run the integration test to verify it passes**

```bash
cargo test -p lab-auth oauth_client_callback_redirects_with_access_denied 2>&1 | tail -10
```

Expected: PASS — response is 303 SEE_OTHER with `error=access_denied&state=client-abc` in the location header.

- [ ] **Step 5: Run the full test suite**

```bash
cargo test -p lab-auth 2>&1 | tail -30
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/lab-auth/src/authorize.rs
git commit -m "feat(auth): enforce email allowlist in OAuth-client branch with RFC 6749 error redirect"
```

---

## Task 6: Update docs and `.env.example`

**Files:**
- Modify: `docs/OAUTH.md`
- Modify: `.env.example`

- [ ] **Step 1: Add the env var to `docs/OAUTH.md`**

In `docs/OAUTH.md`, find the environment variable table under `## Configuration`. Add this row after `LAB_AUTH_ALLOWED_REDIRECT_URIS`:

```markdown
| `LAB_AUTH_ALLOWED_EMAILS` | no | Comma-separated list of Google email addresses permitted to log in. Entries are normalized to lowercase at startup. When empty (the default), any Google account that completes the OAuth flow is allowed. When set, the `email_verified` claim in Google's id_token is also enforced — accounts with unverified email addresses are rejected even if their address is in the list. |
```

- [ ] **Step 2: Add the env var to `.env.example`**

In `.env.example`, find the `LAB_AUTH_ALLOWED_REDIRECT_URIS=` entry. Add directly below it:

```bash
# Optional comma-separated allowlist of Google email addresses permitted to log in.
# Case-insensitive; trailing whitespace is ignored. When unset, any Google account is allowed.
# Example: LAB_AUTH_ALLOWED_EMAILS=alice@gmail.com,bob@example.com
LAB_AUTH_ALLOWED_EMAILS=
```

- [ ] **Step 3: Commit**

```bash
git add docs/OAUTH.md .env.example
git commit -m "docs: document LAB_AUTH_ALLOWED_EMAILS env var"
```

---

## Self-Review

**Spec coverage:**
- ✅ `allowed_emails` field in `AuthConfig` — Task 2
- ✅ `LAB_AUTH_ALLOWED_EMAILS` env var (comma-separated, trimmed, lowercased at startup) — Task 2
- ✅ Empty list = no restriction, backward-compatible — Task 3 unit tests
- ✅ Case-insensitive email comparison — Task 3 unit tests
- ✅ `email_verified` enforced before email comparison — Task 1 + Task 3
- ✅ Browser-login branch: 401 on denial — Task 4
- ✅ OAuth-client branch: RFC 6749 `error=access_denied` redirect on denial — Task 5
- ✅ Rejected email logged as fingerprint (not PII) — Task 3
- ✅ ERROR level for "email claim missing/unverified" (server misconfiguration), WARN for "not in allowlist" (expected rejection) — Task 3
- ✅ Startup warning when allowlist env var is set-but-empty — Task 2
- ✅ `docs/OAUTH.md` updated — Task 6
- ✅ `.env.example` updated — Task 6

**Placeholder scan:** No TBDs, no "implement later". All code blocks are complete.

**Type consistency:**
- `google.email_verified: Option<bool>` threaded from `GoogleIdTokenClaims` → `GoogleExchange` → `check_email_allowlist`
- `allowed_emails: Vec<String>` used consistently across `AuthConfig`, `from_sources`, `Default::default`, and both call sites
- `check_email_allowlist(Option<&str>, Option<bool>, &[String]) -> Result<(), AuthError>` signature matches all call sites: Task 4 passes `google.email.as_deref()`, Task 5 passes `google.email.as_deref()`
