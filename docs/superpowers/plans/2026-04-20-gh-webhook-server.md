# GitHub Webhook Server + PR Comment Monitors Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Rust webhook server that receives GitHub events, debounces PR comment bursts per-PR, generates markdown digests of unresolved review comments, and emits short notification lines into `~/.claude/gh-comments.jsonl` / `~/.claude/gh-ci.jsonl` that Claude Code monitors tail. Extend the `gh-address-comments` skill with matching monitor configs so Claude gets pinged instantly on new PR activity without context-bloat.

**Architecture:** Standalone Rust binary (`tools/gh-webhook-server/`) runs axum on a local port behind Tailscale Funnel. HMAC-SHA256 validates each payload, event type is read from `X-GitHub-Event`, and events are dispatched to handlers. PR comment/review events flow into a per-PR debouncer (30s window); on flush, the server fetches the full thread set from GitHub REST, renders a markdown digest to `~/.claude/gh-comments/<owner>_<repo>_<pr>/latest.md`, and appends one JSONL line summarizing the batch. CI and lifecycle events bypass the debouncer and write directly. A SQLite database dedups comment IDs across webhook redeliveries. Two plugin monitors (`gh-comments`, `gh-ci`) tail the JSONL files and emit one short notification per line.

**Tech Stack:**
- Rust 2024, tokio, axum 0.7, serde / serde_json, hmac + sha2, rusqlite, reqwest (rustls-tls), tracing, anyhow, thiserror
- `gh` CLI for webhook registration
- Tailscale Funnel for public ingress
- Claude Code plugin monitors (`monitors/monitors.json`)

---

## File Structure

**Created files:**

```
tools/gh-webhook-server/
├── Cargo.toml                        # standalone crate (not in lab workspace)
├── README.md                         # setup, Funnel config, env vars
├── .env.example                      # GITHUB_WEBHOOK_SECRET, GITHUB_TOKEN, PORT, DATA_DIR
├── src/
│   ├── main.rs                       # entry: load config, init tracing, build + run server
│   ├── config.rs                     # Config struct, env loading
│   ├── hmac.rs                       # validate_signature(secret, body, header) -> Result<()>
│   ├── events.rs                     # minimal serde types for the 5 event kinds we handle
│   ├── router.rs                     # build_router(state) + /health, /webhook handlers
│   ├── dispatch.rs                   # route event by X-GitHub-Event header to handler
│   ├── dedup.rs                      # sqlite comment-id store
│   ├── github.rs                     # reqwest client: fetch comments / reviews / threads
│   ├── render.rs                     # render markdown digest from fetched data
│   ├── debounce.rs                   # per-PR debouncer with cancellable timers
│   ├── flush.rs                      # flush handler: fetch → render → write .md → append .jsonl
│   ├── ci.rs                         # workflow_run / check_run → gh-ci.jsonl line
│   └── state.rs                      # AppState: Config + Dedup + Debouncer + GhClient
├── scripts/
│   ├── register.sh                   # register webhook on a single repo
│   └── register-all.sh               # register on every repo owned by $GITHUB_USER
├── systemd/
│   └── gh-webhook-server.service     # systemd user unit
└── tests/
    ├── hmac_test.rs                  # HMAC validation vectors
    ├── events_test.rs                # parsing sample payloads
    ├── debounce_test.rs              # debouncer coalesces + fires
    └── integration_test.rs           # end-to-end: POST signed body → assert jsonl + md

skills/gh-address-comments/
└── monitors/
    ├── README.md                     # how the monitors work + how to wire up the server
    └── gh-comments.monitor.sh        # bash wrapper that streams jsonl → pretty lines

monitors/monitors.json                # extended with gh-comments-monitor + gh-ci-monitor
```

**Modified files:**

- `monitors/monitors.json` — add two monitor entries
- `skills/gh-address-comments/SKILL.md` — new "Live notifications" section pointing at monitors

---

## Task 1: Scaffold the `gh-webhook-server` crate

**Files:**
- Create: `tools/gh-webhook-server/Cargo.toml`
- Create: `tools/gh-webhook-server/src/main.rs`
- Create: `tools/gh-webhook-server/.env.example`
- Create: `tools/gh-webhook-server/.gitignore`

- [ ] **Step 1: Create the crate directory**

Run:
```bash
mkdir -p tools/gh-webhook-server/src tools/gh-webhook-server/tests tools/gh-webhook-server/scripts tools/gh-webhook-server/systemd
```

- [ ] **Step 2: Write `Cargo.toml`**

```toml
[package]
name = "gh-webhook-server"
version = "0.1.0"
edition = "2024"
publish = false

[dependencies]
tokio = { version = "1", features = ["rt-multi-thread", "macros", "signal", "time", "sync", "fs", "process"] }
axum = { version = "0.7", features = ["macros"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
hmac = "0.12"
sha2 = "0.10"
subtle = "2"
hex = "0.4"
rusqlite = { version = "0.31", features = ["bundled"] }
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json", "gzip"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }
anyhow = "1"
thiserror = "1"
time = { version = "0.3", features = ["formatting", "macros", "serde-well-known"] }
bytes = "1"
futures = "0.3"
dotenvy = "0.15"

[dev-dependencies]
tower = { version = "0.5", features = ["util"] }
http-body-util = "0.1"
tempfile = "3"
```

- [ ] **Step 3: Write minimal `src/main.rs`**

```rust
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("GH_WEBHOOK_LOG")
                .unwrap_or_else(|_| "gh_webhook_server=info,tower_http=info".into()),
        )
        .init();

    tracing::info!("gh-webhook-server starting");
    Ok(())
}
```

- [ ] **Step 4: Write `.env.example`**

```env
# Secret shared between GitHub and this server (HMAC-SHA256)
GITHUB_WEBHOOK_SECRET=change-me-run-openssl-rand-hex-32

# PAT or fine-grained token with `repo` scope — used to fetch PR threads on flush
# Separate from the webhook secret. Issue via: gh auth token
GITHUB_TOKEN=ghp_...

# GitHub user whose repos get webhooks registered (used by register-all.sh)
GITHUB_USER=jmagar

# Listen address
BIND=127.0.0.1:8787

# Where digests + jsonl streams live
DATA_DIR=/home/jmagar/.claude

# Debounce window per PR, seconds
DEBOUNCE_SECS=30
```

- [ ] **Step 5: Write `.gitignore`**

```
/target
.env
*.sqlite
*.sqlite-journal
```

- [ ] **Step 6: Verify it compiles**

Run:
```bash
cd tools/gh-webhook-server && cargo build
```
Expected: compiles, emits "gh-webhook-server starting" when run.

- [ ] **Step 7: Commit**

```bash
git add tools/gh-webhook-server/
git commit -m "feat(gh-webhook-server): scaffold crate"
```

---

## Task 2: Config loader

**Files:**
- Create: `tools/gh-webhook-server/src/config.rs`
- Modify: `tools/gh-webhook-server/src/main.rs`

- [ ] **Step 1: Write a failing config test**

Create `tools/gh-webhook-server/tests/config_test.rs`:

```rust
use std::env;

#[test]
fn loads_from_env() {
    // SAFETY: tests run serially for this env-touching test via --test-threads=1 if needed.
    env::set_var("GITHUB_WEBHOOK_SECRET", "topsecret");
    env::set_var("GITHUB_TOKEN", "ghp_abc");
    env::set_var("GITHUB_USER", "alice");
    env::set_var("BIND", "127.0.0.1:1234");
    env::set_var("DATA_DIR", "/tmp/gh-test");
    env::set_var("DEBOUNCE_SECS", "45");

    let cfg = gh_webhook_server::config::Config::from_env().unwrap();
    assert_eq!(cfg.webhook_secret, "topsecret");
    assert_eq!(cfg.github_token, "ghp_abc");
    assert_eq!(cfg.github_user, "alice");
    assert_eq!(cfg.bind, "127.0.0.1:1234".parse().unwrap());
    assert_eq!(cfg.data_dir.to_str().unwrap(), "/tmp/gh-test");
    assert_eq!(cfg.debounce_secs, 45);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd tools/gh-webhook-server && cargo test --test config_test`
Expected: fails — `gh_webhook_server` crate does not expose a `lib` target yet.

- [ ] **Step 3: Promote the binary to a library+binary crate**

Edit `Cargo.toml` to add:

```toml
[lib]
name = "gh_webhook_server"
path = "src/lib.rs"

[[bin]]
name = "gh-webhook-server"
path = "src/main.rs"
```

Create `tools/gh-webhook-server/src/lib.rs`:

```rust
pub mod config;
```

- [ ] **Step 4: Implement `src/config.rs`**

```rust
use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};

#[derive(Debug, Clone)]
pub struct Config {
    pub webhook_secret: String,
    pub github_token: String,
    pub github_user: String,
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
    pub debounce_secs: u64,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        fn require(key: &str) -> Result<String> {
            env::var(key).map_err(|_| anyhow!("missing required env var: {key}"))
        }

        let webhook_secret = require("GITHUB_WEBHOOK_SECRET")?;
        let github_token = require("GITHUB_TOKEN")?;
        let github_user = require("GITHUB_USER")?;
        let bind: SocketAddr = env::var("BIND")
            .unwrap_or_else(|_| "127.0.0.1:8787".into())
            .parse()
            .context("BIND must be a valid socket address")?;
        let data_dir: PathBuf = env::var("DATA_DIR")
            .unwrap_or_else(|_| {
                let home = env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                format!("{home}/.claude")
            })
            .into();
        let debounce_secs: u64 = env::var("DEBOUNCE_SECS")
            .unwrap_or_else(|_| "30".into())
            .parse()
            .context("DEBOUNCE_SECS must be an integer")?;

        Ok(Self {
            webhook_secret,
            github_token,
            github_user,
            bind,
            data_dir,
            debounce_secs,
        })
    }
}
```

- [ ] **Step 5: Load `.env` in `main.rs` and print config summary**

Replace `src/main.rs`:

```rust
use anyhow::Result;
use gh_webhook_server::config::Config;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("GH_WEBHOOK_LOG")
                .unwrap_or_else(|_| "gh_webhook_server=info,tower_http=info".into()),
        )
        .init();

    let cfg = Config::from_env()?;
    tracing::info!(
        bind = %cfg.bind,
        data_dir = %cfg.data_dir.display(),
        debounce_secs = cfg.debounce_secs,
        "config loaded"
    );
    Ok(())
}
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test --test config_test -- --test-threads=1`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add tools/gh-webhook-server/
git commit -m "feat(gh-webhook-server): config loader"
```

---

## Task 3: HMAC signature validation

**Files:**
- Create: `tools/gh-webhook-server/src/hmac.rs`
- Create: `tools/gh-webhook-server/tests/hmac_test.rs`
- Modify: `tools/gh-webhook-server/src/lib.rs`

- [ ] **Step 1: Write the failing test with a known vector**

```rust
use gh_webhook_server::hmac::validate_signature;

#[test]
fn accepts_correct_signature() {
    let secret = b"It's a Secret to Everybody";
    let body = b"Hello, World!";
    // sha256 HMAC of body with this secret, computed offline:
    // echo -n "Hello, World!" | openssl dgst -sha256 -hmac "It's a Secret to Everybody"
    let header = "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";
    validate_signature(secret, body, header).unwrap();
}

#[test]
fn rejects_wrong_signature() {
    let secret = b"It's a Secret to Everybody";
    let body = b"Hello, World!";
    let header = "sha256=0000000000000000000000000000000000000000000000000000000000000000";
    assert!(validate_signature(secret, body, header).is_err());
}

#[test]
fn rejects_missing_prefix() {
    let secret = b"x";
    let body = b"y";
    assert!(validate_signature(secret, body, "abc").is_err());
}

#[test]
fn rejects_non_hex() {
    let secret = b"x";
    let body = b"y";
    assert!(validate_signature(secret, body, "sha256=zzzz").is_err());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test hmac_test`
Expected: fails — module not defined.

- [ ] **Step 3: Implement `src/hmac.rs`**

```rust
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Error)]
pub enum HmacError {
    #[error("missing sha256= prefix in signature header")]
    MissingPrefix,
    #[error("signature header is not valid hex")]
    InvalidHex,
    #[error("signature mismatch")]
    Mismatch,
    #[error("hmac init failed")]
    KeyError,
}

/// Validate the `X-Hub-Signature-256` header against the body.
/// Header format is `sha256=<hex>`.
pub fn validate_signature(secret: &[u8], body: &[u8], header: &str) -> Result<(), HmacError> {
    let hex_sig = header.strip_prefix("sha256=").ok_or(HmacError::MissingPrefix)?;
    let provided = hex::decode(hex_sig).map_err(|_| HmacError::InvalidHex)?;

    let mut mac = HmacSha256::new_from_slice(secret).map_err(|_| HmacError::KeyError)?;
    mac.update(body);
    let expected = mac.finalize().into_bytes();

    if provided.ct_eq(expected.as_slice()).into() {
        Ok(())
    } else {
        Err(HmacError::Mismatch)
    }
}
```

- [ ] **Step 4: Expose the module**

Edit `src/lib.rs`:
```rust
pub mod config;
pub mod hmac;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --test hmac_test`
Expected: 4 passed.

- [ ] **Step 6: Commit**

```bash
git add tools/gh-webhook-server/
git commit -m "feat(gh-webhook-server): HMAC-SHA256 signature validation"
```

---

## Task 4: Event payload types

**Files:**
- Create: `tools/gh-webhook-server/src/events.rs`
- Create: `tools/gh-webhook-server/tests/events_test.rs`
- Create: `tools/gh-webhook-server/tests/fixtures/` (sample payloads)
- Modify: `tools/gh-webhook-server/src/lib.rs`

We define a minimal subset of GitHub's payload schema — just the fields we need. Each event type is a separate struct; the router picks one based on `X-GitHub-Event`.

- [ ] **Step 1: Save sample payloads**

Download minimal payload fixtures (trim GitHub's full examples to just the fields we use). Create `tools/gh-webhook-server/tests/fixtures/issue_comment.json`:

```json
{
  "action": "created",
  "repository": {
    "full_name": "jmagar/lab",
    "name": "lab",
    "owner": { "login": "jmagar" }
  },
  "issue": {
    "number": 42,
    "pull_request": { "url": "https://api.github.com/repos/jmagar/lab/pulls/42" }
  },
  "comment": {
    "id": 123456789,
    "user": { "login": "reviewer" },
    "body": "nit: rename this"
  }
}
```

Create `tools/gh-webhook-server/tests/fixtures/pr_review_comment.json`:

```json
{
  "action": "created",
  "repository": {
    "full_name": "jmagar/lab",
    "name": "lab",
    "owner": { "login": "jmagar" }
  },
  "pull_request": { "number": 42, "head": { "ref": "fix/auth" } },
  "comment": { "id": 987654321, "user": { "login": "reviewer" }, "body": "fix this" }
}
```

Create `tools/gh-webhook-server/tests/fixtures/pull_request_opened.json`:

```json
{
  "action": "opened",
  "repository": {
    "full_name": "jmagar/lab",
    "name": "lab",
    "owner": { "login": "jmagar" }
  },
  "pull_request": {
    "number": 99,
    "title": "New feature",
    "head": { "ref": "feat/new" },
    "html_url": "https://github.com/jmagar/lab/pull/99"
  }
}
```

Create `tools/gh-webhook-server/tests/fixtures/workflow_run_failure.json`:

```json
{
  "action": "completed",
  "repository": {
    "full_name": "jmagar/lab",
    "name": "lab",
    "owner": { "login": "jmagar" }
  },
  "workflow_run": {
    "id": 555,
    "name": "ci",
    "conclusion": "failure",
    "head_branch": "fix/auth",
    "pull_requests": [{ "number": 42 }],
    "html_url": "https://github.com/jmagar/lab/actions/runs/555"
  }
}
```

- [ ] **Step 2: Write the failing parse test**

Create `tests/events_test.rs`:

```rust
use gh_webhook_server::events::{IssueCommentEvent, PrReviewCommentEvent, PullRequestEvent, WorkflowRunEvent};

#[test]
fn parses_issue_comment_with_pr() {
    let body = include_str!("fixtures/issue_comment.json");
    let evt: IssueCommentEvent = serde_json::from_str(body).unwrap();
    assert_eq!(evt.repository.full_name, "jmagar/lab");
    assert_eq!(evt.issue.number, 42);
    assert!(evt.issue.pull_request.is_some());
    assert_eq!(evt.comment.id, 123456789);
}

#[test]
fn parses_pr_review_comment() {
    let body = include_str!("fixtures/pr_review_comment.json");
    let evt: PrReviewCommentEvent = serde_json::from_str(body).unwrap();
    assert_eq!(evt.pull_request.number, 42);
    assert_eq!(evt.pull_request.head.r#ref, "fix/auth");
    assert_eq!(evt.comment.id, 987654321);
}

#[test]
fn parses_pull_request_opened() {
    let body = include_str!("fixtures/pull_request_opened.json");
    let evt: PullRequestEvent = serde_json::from_str(body).unwrap();
    assert_eq!(evt.action, "opened");
    assert_eq!(evt.pull_request.number, 99);
    assert_eq!(evt.pull_request.html_url, "https://github.com/jmagar/lab/pull/99");
}

#[test]
fn parses_workflow_run_failure() {
    let body = include_str!("fixtures/workflow_run_failure.json");
    let evt: WorkflowRunEvent = serde_json::from_str(body).unwrap();
    assert_eq!(evt.workflow_run.conclusion.as_deref(), Some("failure"));
    assert_eq!(evt.workflow_run.pull_requests.len(), 1);
    assert_eq!(evt.workflow_run.pull_requests[0].number, 42);
}
```

- [ ] **Step 3: Run — expect failure**

Run: `cargo test --test events_test`
Expected: fails — `events` module not defined.

- [ ] **Step 4: Implement `src/events.rs`**

```rust
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Owner {
    pub login: String,
}

#[derive(Debug, Deserialize)]
pub struct Repository {
    pub full_name: String,
    pub name: String,
    pub owner: Owner,
}

#[derive(Debug, Deserialize)]
pub struct User {
    pub login: String,
}

#[derive(Debug, Deserialize)]
pub struct Comment {
    pub id: i64,
    pub user: User,
    pub body: String,
}

#[derive(Debug, Deserialize)]
pub struct PrRef {
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct Issue {
    pub number: u64,
    pub pull_request: Option<PrRef>,
}

#[derive(Debug, Deserialize)]
pub struct Head {
    pub r#ref: String,
}

#[derive(Debug, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub html_url: String,
    pub head: Head,
}

#[derive(Debug, Deserialize)]
pub struct IssueCommentEvent {
    pub action: String,
    pub repository: Repository,
    pub issue: Issue,
    pub comment: Comment,
}

#[derive(Debug, Deserialize)]
pub struct PrReviewCommentEvent {
    pub action: String,
    pub repository: Repository,
    pub pull_request: PullRequest,
    pub comment: Comment,
}

#[derive(Debug, Deserialize)]
pub struct PullRequestReviewEvent {
    pub action: String,
    pub repository: Repository,
    pub pull_request: PullRequest,
    pub review: Review,
}

#[derive(Debug, Deserialize)]
pub struct Review {
    pub id: i64,
    pub user: User,
    pub state: String,
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Deserialize)]
pub struct PullRequestEvent {
    pub action: String,
    pub repository: Repository,
    pub pull_request: PullRequest,
}

#[derive(Debug, Deserialize)]
pub struct PrNumberRef {
    pub number: u64,
}

#[derive(Debug, Deserialize)]
pub struct WorkflowRun {
    pub id: u64,
    pub name: String,
    pub conclusion: Option<String>,
    pub head_branch: String,
    #[serde(default)]
    pub pull_requests: Vec<PrNumberRef>,
    pub html_url: String,
}

#[derive(Debug, Deserialize)]
pub struct WorkflowRunEvent {
    pub action: String,
    pub repository: Repository,
    pub workflow_run: WorkflowRun,
}

#[derive(Debug, Deserialize)]
pub struct CheckRun {
    pub id: u64,
    pub name: String,
    pub conclusion: Option<String>,
    pub html_url: String,
    #[serde(default)]
    pub pull_requests: Vec<PrNumberRef>,
}

#[derive(Debug, Deserialize)]
pub struct CheckRunEvent {
    pub action: String,
    pub repository: Repository,
    pub check_run: CheckRun,
}

#[derive(Debug, Deserialize)]
pub struct IssuesEvent {
    pub action: String,
    pub repository: Repository,
    pub issue: IssueFull,
}

#[derive(Debug, Deserialize)]
pub struct IssueFull {
    pub number: u64,
    pub title: String,
    pub html_url: String,
    pub user: User,
}
```

- [ ] **Step 5: Expose module**

Edit `src/lib.rs`:
```rust
pub mod config;
pub mod events;
pub mod hmac;
```

- [ ] **Step 6: Run — expect pass**

Run: `cargo test --test events_test`
Expected: 4 passed.

- [ ] **Step 7: Commit**

```bash
git add tools/gh-webhook-server/
git commit -m "feat(gh-webhook-server): event payload types + parsing tests"
```

---

## Task 5: SQLite dedup store

**Files:**
- Create: `tools/gh-webhook-server/src/dedup.rs`
- Create: `tools/gh-webhook-server/tests/dedup_test.rs`
- Modify: `tools/gh-webhook-server/src/lib.rs`

- [ ] **Step 1: Write the failing test**

```rust
use gh_webhook_server::dedup::Dedup;
use tempfile::tempdir;

#[test]
fn records_and_detects_duplicates() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("dedup.sqlite");
    let dedup = Dedup::open(&db).unwrap();

    assert!(dedup.mark_seen("comment", 1).unwrap(), "first insert should return true (new)");
    assert!(!dedup.mark_seen("comment", 1).unwrap(), "second insert should return false (dup)");
    assert!(dedup.mark_seen("comment", 2).unwrap());
    assert!(dedup.mark_seen("review", 1).unwrap(), "same id different kind is fresh");
}
```

- [ ] **Step 2: Run — expect failure**

Run: `cargo test --test dedup_test`
Expected: fails — module not defined.

- [ ] **Step 3: Implement `src/dedup.rs`**

```rust
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

pub struct Dedup {
    conn: Mutex<Connection>,
}

impl Dedup {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path).context("open dedup db")?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS seen (
                kind TEXT NOT NULL,
                id   INTEGER NOT NULL,
                ts   INTEGER NOT NULL DEFAULT (strftime('%s','now')),
                PRIMARY KEY (kind, id)
            );
            "#,
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// Returns true if the (kind, id) pair is newly recorded,
    /// false if it was already present.
    pub fn mark_seen(&self, kind: &str, id: i64) -> Result<bool> {
        let conn = self.conn.lock().expect("dedup mutex");
        let inserted = conn
            .execute(
                "INSERT OR IGNORE INTO seen (kind, id) VALUES (?1, ?2)",
                params![kind, id],
            )
            .context("insert seen")?;
        Ok(inserted == 1)
    }

    #[allow(dead_code)]
    pub fn has_seen(&self, kind: &str, id: i64) -> Result<bool> {
        let conn = self.conn.lock().expect("dedup mutex");
        let row: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM seen WHERE kind = ?1 AND id = ?2",
                params![kind, id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(row.is_some())
    }
}
```

- [ ] **Step 4: Expose module**

Edit `src/lib.rs`:
```rust
pub mod config;
pub mod dedup;
pub mod events;
pub mod hmac;
```

- [ ] **Step 5: Run — expect pass**

Run: `cargo test --test dedup_test`
Expected: 1 passed.

- [ ] **Step 6: Commit**

```bash
git add tools/gh-webhook-server/
git commit -m "feat(gh-webhook-server): sqlite-backed dedup for webhook redeliveries"
```

---

## Task 6: GitHub REST client (fetch PR threads)

**Files:**
- Create: `tools/gh-webhook-server/src/github.rs`
- Modify: `tools/gh-webhook-server/src/lib.rs`

This module wraps reqwest to fetch the three data sources we need on flush:
1. `/repos/{owner}/{repo}/pulls/{n}` — PR metadata
2. `/repos/{owner}/{repo}/pulls/{n}/comments` — inline review comments
3. `/repos/{owner}/{repo}/issues/{n}/comments` — conversation comments

We do not test this with a live API in CI — instead we test the rendering layer (next task) against fixture data.

- [ ] **Step 1: Define shared data types and the client**

Create `src/github.rs`:

```rust
use anyhow::{Context, Result};
use reqwest::{Client, header};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct PrDetail {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub html_url: String,
    pub user: User,
    pub head: Head,
    pub base: Base,
    pub draft: bool,
    pub merged: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub login: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Head {
    #[serde(rename = "ref")]
    pub branch: String,
    pub sha: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Base {
    #[serde(rename = "ref")]
    pub branch: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReviewComment {
    pub id: i64,
    pub user: User,
    pub body: String,
    pub path: String,
    pub line: Option<u32>,
    pub html_url: String,
    pub in_reply_to_id: Option<i64>,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IssueComment {
    pub id: i64,
    pub user: User,
    pub body: String,
    pub html_url: String,
    pub created_at: String,
}

pub struct GhClient {
    http: Client,
}

impl GhClient {
    pub fn new(token: &str) -> Result<Self> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_str(&format!("Bearer {token}"))?,
        );
        headers.insert(header::ACCEPT, header::HeaderValue::from_static("application/vnd.github+json"));
        headers.insert("X-GitHub-Api-Version", header::HeaderValue::from_static("2022-11-28"));
        headers.insert(header::USER_AGENT, header::HeaderValue::from_static("gh-webhook-server/0.1"));

        let http = Client::builder()
            .default_headers(headers)
            .gzip(true)
            .build()
            .context("build reqwest client")?;
        Ok(Self { http })
    }

    pub async fn pr_detail(&self, owner: &str, repo: &str, number: u64) -> Result<PrDetail> {
        let url = format!("https://api.github.com/repos/{owner}/{repo}/pulls/{number}");
        self.http
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .json::<PrDetail>()
            .await
            .context("parse pr detail")
    }

    pub async fn review_comments(&self, owner: &str, repo: &str, number: u64) -> Result<Vec<ReviewComment>> {
        let url = format!("https://api.github.com/repos/{owner}/{repo}/pulls/{number}/comments?per_page=100");
        self.http
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .json::<Vec<ReviewComment>>()
            .await
            .context("parse review comments")
    }

    pub async fn issue_comments(&self, owner: &str, repo: &str, number: u64) -> Result<Vec<IssueComment>> {
        let url = format!("https://api.github.com/repos/{owner}/{repo}/issues/{number}/comments?per_page=100");
        self.http
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .json::<Vec<IssueComment>>()
            .await
            .context("parse issue comments")
    }
}
```

- [ ] **Step 2: Expose module**

Edit `src/lib.rs`:
```rust
pub mod config;
pub mod dedup;
pub mod events;
pub mod github;
pub mod hmac;
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build`
Expected: compiles without warnings.

- [ ] **Step 4: Commit**

```bash
git add tools/gh-webhook-server/
git commit -m "feat(gh-webhook-server): github REST client for pr/comment fetch"
```

---

## Task 7: Markdown renderer

**Files:**
- Create: `tools/gh-webhook-server/src/render.rs`
- Create: `tools/gh-webhook-server/tests/render_test.rs`
- Modify: `tools/gh-webhook-server/src/lib.rs`

Renders a single PR's state into a markdown digest Claude can read in one tool call. Groups review comments into threads by `in_reply_to_id`.

- [ ] **Step 1: Write the failing test**

Create `tests/render_test.rs`:

```rust
use gh_webhook_server::github::{Base, Head, IssueComment, PrDetail, ReviewComment, User};
use gh_webhook_server::render::render_digest;

fn pr() -> PrDetail {
    PrDetail {
        number: 42,
        title: "Add feature X".into(),
        state: "open".into(),
        html_url: "https://github.com/jmagar/lab/pull/42".into(),
        user: User { login: "jmagar".into() },
        head: Head { branch: "feat/x".into(), sha: "abc1234".into() },
        base: Base { branch: "main".into() },
        draft: false,
        merged: Some(false),
    }
}

fn rc(id: i64, body: &str, reply_to: Option<i64>) -> ReviewComment {
    ReviewComment {
        id,
        user: User { login: "reviewer".into() },
        body: body.into(),
        path: "src/lib.rs".into(),
        line: Some(10),
        html_url: format!("https://github.com/jmagar/lab/pull/42#discussion_r{id}"),
        in_reply_to_id: reply_to,
        created_at: "2026-04-20T12:00:00Z".into(),
    }
}

#[test]
fn renders_digest_with_threads_and_conversation() {
    let pr = pr();
    let review = vec![
        rc(1, "rename this", None),
        rc(2, "agree", Some(1)),
        rc(3, "separate concern", None),
    ];
    let issue = vec![IssueComment {
        id: 999,
        user: User { login: "bot".into() },
        body: "LGTM from CI".into(),
        html_url: "https://github.com/jmagar/lab/pull/42#issuecomment-999".into(),
        created_at: "2026-04-20T11:00:00Z".into(),
    }];

    let md = render_digest(&pr, &review, &issue);
    assert!(md.contains("# PR #42 — Add feature X"));
    assert!(md.contains("**Branch:** `feat/x` → `main`"));
    assert!(md.contains("## Inline review threads (2)"));
    assert!(md.contains("rename this"));
    assert!(md.contains("agree"));
    assert!(md.contains("separate concern"));
    assert!(md.contains("## Conversation (1)"));
    assert!(md.contains("LGTM from CI"));
}

#[test]
fn omits_empty_sections() {
    let md = render_digest(&pr(), &[], &[]);
    assert!(md.contains("# PR #42"));
    assert!(!md.contains("## Inline review threads"));
    assert!(!md.contains("## Conversation"));
}
```

- [ ] **Step 2: Run — expect failure**

Run: `cargo test --test render_test`
Expected: fails — `render` module not defined.

- [ ] **Step 3: Implement `src/render.rs`**

```rust
use std::collections::BTreeMap;
use std::fmt::Write;

use crate::github::{IssueComment, PrDetail, ReviewComment};

pub fn render_digest(
    pr: &PrDetail,
    review_comments: &[ReviewComment],
    issue_comments: &[IssueComment],
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# PR #{} — {}", pr.number, pr.title);
    let _ = writeln!(out);
    let _ = writeln!(out, "**URL:** {}", pr.html_url);
    let _ = writeln!(
        out,
        "**Branch:** `{}` → `{}`",
        pr.head.branch, pr.base.branch
    );
    let _ = writeln!(out, "**Head SHA:** `{}`", pr.head.sha);
    let _ = writeln!(out, "**State:** {}", pr.state);
    let _ = writeln!(out, "**Author:** @{}", pr.user.login);
    let _ = writeln!(out);

    let threads = group_threads(review_comments);
    if !threads.is_empty() {
        let _ = writeln!(out, "## Inline review threads ({})", threads.len());
        let _ = writeln!(out);
        for (root_id, thread) in &threads {
            let root = thread.first().unwrap();
            let _ = writeln!(
                out,
                "### `{}`{}  _thread {}_",
                root.path,
                root.line.map(|l| format!(":{l}")).unwrap_or_default(),
                root_id
            );
            for c in thread {
                let _ = writeln!(
                    out,
                    "- **@{}** [{}]({}):\n  {}",
                    c.user.login,
                    c.created_at,
                    c.html_url,
                    indent_body(&c.body)
                );
            }
            let _ = writeln!(out);
        }
    }

    if !issue_comments.is_empty() {
        let _ = writeln!(out, "## Conversation ({})", issue_comments.len());
        let _ = writeln!(out);
        for c in issue_comments {
            let _ = writeln!(
                out,
                "- **@{}** [{}]({}):\n  {}",
                c.user.login,
                c.created_at,
                c.html_url,
                indent_body(&c.body)
            );
        }
    }

    out
}

fn group_threads(comments: &[ReviewComment]) -> BTreeMap<i64, Vec<&ReviewComment>> {
    let mut threads: BTreeMap<i64, Vec<&ReviewComment>> = BTreeMap::new();
    for c in comments {
        let root = c.in_reply_to_id.unwrap_or(c.id);
        threads.entry(root).or_default().push(c);
    }
    for v in threads.values_mut() {
        v.sort_by_key(|c| c.id);
    }
    threads
}

fn indent_body(body: &str) -> String {
    body.lines().collect::<Vec<_>>().join("\n  ")
}
```

- [ ] **Step 4: Expose module**

Edit `src/lib.rs`:
```rust
pub mod config;
pub mod dedup;
pub mod events;
pub mod github;
pub mod hmac;
pub mod render;
```

- [ ] **Step 5: Run — expect pass**

Run: `cargo test --test render_test`
Expected: 2 passed.

- [ ] **Step 6: Commit**

```bash
git add tools/gh-webhook-server/
git commit -m "feat(gh-webhook-server): markdown digest renderer"
```

---

## Task 8: Per-PR debouncer

**Files:**
- Create: `tools/gh-webhook-server/src/debounce.rs`
- Create: `tools/gh-webhook-server/tests/debounce_test.rs`
- Modify: `tools/gh-webhook-server/src/lib.rs`

Debouncer keeps a `HashMap<PrKey, BatchState>`. Each `push()` cancels the existing flush task and schedules a new one `debounce_secs` in the future. The flush callback is passed the accumulated events.

- [ ] **Step 1: Write the failing test**

Create `tests/debounce_test.rs`:

```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use gh_webhook_server::debounce::{Debouncer, PrKey};
use tokio::sync::Mutex;

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn coalesces_events_inside_window() {
    let flushes: Arc<Mutex<Vec<(PrKey, Vec<u32>)>>> = Arc::new(Mutex::new(Vec::new()));
    let flushes_c = flushes.clone();

    let debouncer = Debouncer::<u32>::new(Duration::from_secs(30), move |key, items| {
        let flushes_c = flushes_c.clone();
        async move {
            flushes_c.lock().await.push((key, items));
        }
    });

    let key = PrKey::new("alice", "repo", 7);
    debouncer.push(key.clone(), 1).await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    debouncer.push(key.clone(), 2).await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    debouncer.push(key.clone(), 3).await;

    // Not yet — 10s into a 30s window
    assert!(flushes.lock().await.is_empty());

    tokio::time::sleep(Duration::from_secs(31)).await;

    let got = flushes.lock().await.clone();
    assert_eq!(got.len(), 1, "one batch");
    assert_eq!(got[0].0, key);
    assert_eq!(got[0].1, vec![1, 2, 3]);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn separate_prs_flush_independently() {
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_c = counter.clone();

    let debouncer = Debouncer::<u32>::new(Duration::from_secs(30), move |_, _| {
        let counter_c = counter_c.clone();
        async move {
            counter_c.fetch_add(1, Ordering::SeqCst);
        }
    });

    debouncer.push(PrKey::new("a", "b", 1), 10).await;
    debouncer.push(PrKey::new("a", "b", 2), 20).await;
    tokio::time::sleep(Duration::from_secs(31)).await;

    assert_eq!(counter.load(Ordering::SeqCst), 2);
}
```

- [ ] **Step 2: Run — expect failure**

Run: `cargo test --test debounce_test`
Expected: fails — module not defined.

- [ ] **Step 3: Implement `src/debounce.rs`**

```rust
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::hash::Hash;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::task::JoinHandle;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PrKey {
    pub owner: String,
    pub repo: String,
    pub number: u64,
}

impl PrKey {
    pub fn new(owner: impl Into<String>, repo: impl Into<String>, number: u64) -> Self {
        Self {
            owner: owner.into(),
            repo: repo.into(),
            number,
        }
    }
}

impl fmt::Display for PrKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}#{}", self.owner, self.repo, self.number)
    }
}

struct Batch<T> {
    items: Vec<T>,
    timer: Option<JoinHandle<()>>,
}

type FlushMap<K, T> = Arc<Mutex<HashMap<K, Batch<T>>>>;

pub struct Debouncer<T: Send + 'static> {
    inner: Inner<T>,
}

struct Inner<T: Send + 'static> {
    window: Duration,
    batches: FlushMap<PrKey, T>,
    flush_fn: Arc<dyn FlushFn<T>>,
}

impl<T: Send + 'static> Clone for Inner<T> {
    fn clone(&self) -> Self {
        Self {
            window: self.window,
            batches: self.batches.clone(),
            flush_fn: self.flush_fn.clone(),
        }
    }
}

pub trait FlushFn<T>: Send + Sync + 'static {
    fn call(&self, key: PrKey, items: Vec<T>) -> futures::future::BoxFuture<'static, ()>;
}

impl<T, F, Fut> FlushFn<T> for F
where
    T: Send + 'static,
    F: Fn(PrKey, Vec<T>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    fn call(&self, key: PrKey, items: Vec<T>) -> futures::future::BoxFuture<'static, ()> {
        Box::pin((self)(key, items))
    }
}

impl<T: Send + 'static> Debouncer<T> {
    pub fn new<F, Fut>(window: Duration, flush: F) -> Self
    where
        F: Fn(PrKey, Vec<T>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        Self {
            inner: Inner {
                window,
                batches: Arc::new(Mutex::new(HashMap::new())),
                flush_fn: Arc::new(flush),
            },
        }
    }

    pub async fn push(&self, key: PrKey, item: T) {
        let mut guard = self.inner.batches.lock().await;
        let batch = guard.entry(key.clone()).or_insert_with(|| Batch {
            items: Vec::new(),
            timer: None,
        });
        batch.items.push(item);

        if let Some(h) = batch.timer.take() {
            h.abort();
        }

        let inner = self.inner.clone();
        let timer_key = key.clone();
        let window = self.inner.window;
        let handle = tokio::spawn(async move {
            tokio::time::sleep(window).await;
            let items = {
                let mut guard = inner.batches.lock().await;
                match guard.remove(&timer_key) {
                    Some(batch) => batch.items,
                    None => return,
                }
            };
            inner.flush_fn.call(timer_key, items).await;
        });
        batch.timer = Some(handle);
    }
}
```

- [ ] **Step 4: Expose module**

Edit `src/lib.rs`:
```rust
pub mod config;
pub mod debounce;
pub mod dedup;
pub mod events;
pub mod github;
pub mod hmac;
pub mod render;
```

- [ ] **Step 5: Run — expect pass**

Run: `cargo test --test debounce_test`
Expected: 2 passed.

- [ ] **Step 6: Commit**

```bash
git add tools/gh-webhook-server/
git commit -m "feat(gh-webhook-server): per-PR debouncer with cancellable timers"
```

---

## Task 9: Flush handler (fetch → render → write → notify)

**Files:**
- Create: `tools/gh-webhook-server/src/flush.rs`
- Modify: `tools/gh-webhook-server/src/lib.rs`

On debounce flush we:
1. Call `GhClient::pr_detail`, `review_comments`, `issue_comments`
2. Render markdown
3. Create `<data_dir>/gh-comments/<owner>_<repo>_<number>/` if missing
4. Write `latest.md` + timestamped archive copy
5. Append a JSONL notification to `<data_dir>/gh-comments.jsonl`

- [ ] **Step 1: Implement `src/flush.rs`**

```rust
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::Serialize;
use time::OffsetDateTime;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::debounce::PrKey;
use crate::github::GhClient;
use crate::render::render_digest;

#[derive(Debug, Clone, Serialize)]
pub struct NotificationLine {
    pub ts: String,
    pub repo: String,
    pub pr: u64,
    pub branch: String,
    pub new_events: usize,
    pub review_threads: usize,
    pub conversation: usize,
    pub path: String,
    pub url: String,
}

pub struct Flusher {
    client: Arc<GhClient>,
    data_dir: PathBuf,
}

impl Flusher {
    pub fn new(client: Arc<GhClient>, data_dir: PathBuf) -> Self {
        Self { client, data_dir }
    }

    pub async fn flush(&self, key: PrKey, new_events: usize) -> Result<()> {
        let pr = self
            .client
            .pr_detail(&key.owner, &key.repo, key.number)
            .await
            .with_context(|| format!("fetch pr detail for {key}"))?;
        let review = self
            .client
            .review_comments(&key.owner, &key.repo, key.number)
            .await
            .with_context(|| format!("fetch review comments for {key}"))?;
        let issue = self
            .client
            .issue_comments(&key.owner, &key.repo, key.number)
            .await
            .with_context(|| format!("fetch issue comments for {key}"))?;

        let md = render_digest(&pr, &review, &issue);

        let pr_dir = self.data_dir.join("gh-comments").join(format!(
            "{}_{}_{}",
            key.owner, key.repo, key.number
        ));
        fs::create_dir_all(&pr_dir).await?;

        let now = OffsetDateTime::now_utc();
        let stamp = now
            .format(&time::macros::format_description!(
                "[year][month][day]T[hour][minute][second]Z"
            ))
            .unwrap_or_else(|_| "unknown".into());

        let latest = pr_dir.join("latest.md");
        let archive = pr_dir.join(format!("{stamp}.md"));

        fs::write(&latest, md.as_bytes()).await?;
        fs::write(&archive, md.as_bytes()).await?;

        let ts = now
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "unknown".into());

        let line = NotificationLine {
            ts,
            repo: format!("{}/{}", key.owner, key.repo),
            pr: key.number,
            branch: pr.head.branch.clone(),
            new_events,
            review_threads: count_threads(&review),
            conversation: issue.len(),
            path: latest.to_string_lossy().into(),
            url: pr.html_url.clone(),
        };

        append_jsonl(&self.data_dir.join("gh-comments.jsonl"), &line).await?;

        tracing::info!(
            pr = %key,
            new_events = line.new_events,
            threads = line.review_threads,
            "flush complete"
        );
        Ok(())
    }
}

fn count_threads(comments: &[crate::github::ReviewComment]) -> usize {
    use std::collections::BTreeSet;
    let roots: BTreeSet<i64> = comments
        .iter()
        .map(|c| c.in_reply_to_id.unwrap_or(c.id))
        .collect();
    roots.len()
}

pub async fn append_jsonl(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    let mut line = serde_json::to_string(value)?;
    line.push('\n');
    file.write_all(line.as_bytes()).await?;
    file.flush().await?;
    Ok(())
}
```

- [ ] **Step 2: Expose module**

Edit `src/lib.rs`:
```rust
pub mod config;
pub mod debounce;
pub mod dedup;
pub mod events;
pub mod flush;
pub mod github;
pub mod hmac;
pub mod render;
```

- [ ] **Step 3: Compile check**

Run: `cargo build`
Expected: compiles.

- [ ] **Step 4: Commit**

```bash
git add tools/gh-webhook-server/
git commit -m "feat(gh-webhook-server): flush pipeline fetches, renders, writes digest + jsonl"
```

---

## Task 10: CI-event notifier

**Files:**
- Create: `tools/gh-webhook-server/src/ci.rs`
- Modify: `tools/gh-webhook-server/src/lib.rs`

CI events (`workflow_run.completed` with `conclusion=failure`, `check_run.completed` with `conclusion=failure`) write immediately to `<data_dir>/gh-ci.jsonl` — no debounce. One line per failed run.

- [ ] **Step 1: Implement `src/ci.rs`**

```rust
use std::path::PathBuf;

use anyhow::Result;
use serde::Serialize;
use time::OffsetDateTime;

use crate::events::{CheckRunEvent, WorkflowRunEvent};
use crate::flush::append_jsonl;

#[derive(Debug, Serialize)]
pub struct CiLine {
    pub ts: String,
    pub repo: String,
    pub pr: Option<u64>,
    pub branch: String,
    pub kind: &'static str,
    pub name: String,
    pub conclusion: String,
    pub url: String,
}

pub async fn notify_workflow(data_dir: &PathBuf, evt: &WorkflowRunEvent) -> Result<()> {
    if evt.action != "completed" {
        return Ok(());
    }
    let Some(conclusion) = evt.workflow_run.conclusion.as_deref() else {
        return Ok(());
    };
    if conclusion == "success" || conclusion == "skipped" || conclusion == "neutral" {
        return Ok(());
    }
    let pr = evt.workflow_run.pull_requests.first().map(|p| p.number);
    let line = CiLine {
        ts: now_rfc3339(),
        repo: evt.repository.full_name.clone(),
        pr,
        branch: evt.workflow_run.head_branch.clone(),
        kind: "workflow_run",
        name: evt.workflow_run.name.clone(),
        conclusion: conclusion.into(),
        url: evt.workflow_run.html_url.clone(),
    };
    append_jsonl(&data_dir.join("gh-ci.jsonl"), &line).await?;
    Ok(())
}

pub async fn notify_check_run(data_dir: &PathBuf, evt: &CheckRunEvent) -> Result<()> {
    if evt.action != "completed" {
        return Ok(());
    }
    let Some(conclusion) = evt.check_run.conclusion.as_deref() else {
        return Ok(());
    };
    if conclusion == "success" || conclusion == "skipped" || conclusion == "neutral" {
        return Ok(());
    }
    let pr = evt.check_run.pull_requests.first().map(|p| p.number);
    let line = CiLine {
        ts: now_rfc3339(),
        repo: evt.repository.full_name.clone(),
        pr,
        branch: String::new(),
        kind: "check_run",
        name: evt.check_run.name.clone(),
        conclusion: conclusion.into(),
        url: evt.check_run.html_url.clone(),
    };
    append_jsonl(&data_dir.join("gh-ci.jsonl"), &line).await?;
    Ok(())
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "unknown".into())
}
```

- [ ] **Step 2: Expose module**

Edit `src/lib.rs`:
```rust
pub mod ci;
pub mod config;
pub mod debounce;
pub mod dedup;
pub mod events;
pub mod flush;
pub mod github;
pub mod hmac;
pub mod render;
```

- [ ] **Step 3: Compile**

Run: `cargo build`
Expected: compiles.

- [ ] **Step 4: Commit**

```bash
git add tools/gh-webhook-server/
git commit -m "feat(gh-webhook-server): CI event notifier → gh-ci.jsonl"
```

---

## Task 11: AppState + event dispatch

**Files:**
- Create: `tools/gh-webhook-server/src/state.rs`
- Create: `tools/gh-webhook-server/src/dispatch.rs`
- Modify: `tools/gh-webhook-server/src/lib.rs`

- [ ] **Step 1: Implement `src/state.rs`**

```rust
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

use crate::config::Config;
use crate::debounce::{Debouncer, PrKey};
use crate::dedup::Dedup;
use crate::flush::Flusher;
use crate::github::GhClient;

/// Marker enum for events pushed into the debouncer.
#[derive(Debug, Clone)]
pub enum BatchedEvent {
    Comment,
    Review,
    Synchronize,
}

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub dedup: Arc<Dedup>,
    pub debouncer: Arc<Debouncer<BatchedEvent>>,
    pub data_dir: PathBuf,
}

impl AppState {
    pub fn new(cfg: Config) -> Result<Self> {
        let data_dir = cfg.data_dir.clone();
        let dedup_path = data_dir.join("gh-webhook-dedup.sqlite");
        std::fs::create_dir_all(&data_dir)?;
        let dedup = Arc::new(Dedup::open(&dedup_path)?);
        let gh = Arc::new(GhClient::new(&cfg.github_token)?);
        let flusher = Arc::new(Flusher::new(gh.clone(), data_dir.clone()));

        let flusher_c = flusher.clone();
        let debouncer = Arc::new(Debouncer::new(
            Duration::from_secs(cfg.debounce_secs),
            move |key: PrKey, items: Vec<BatchedEvent>| {
                let flusher_c = flusher_c.clone();
                async move {
                    if let Err(err) = flusher_c.flush(key.clone(), items.len()).await {
                        tracing::error!(pr = %key, error = %err, "flush failed");
                    }
                }
            },
        ));

        Ok(Self {
            cfg: Arc::new(cfg),
            dedup,
            debouncer,
            data_dir,
        })
    }
}
```

- [ ] **Step 2: Implement `src/dispatch.rs`**

```rust
use anyhow::{Context, Result};

use crate::ci::{notify_check_run, notify_workflow};
use crate::events::{
    CheckRunEvent, IssueCommentEvent, IssuesEvent, PrReviewCommentEvent, PullRequestEvent,
    PullRequestReviewEvent, WorkflowRunEvent,
};
use crate::debounce::PrKey;
use crate::flush::append_jsonl;
use crate::state::{AppState, BatchedEvent};

pub async fn handle(state: &AppState, event_name: &str, body: &[u8]) -> Result<()> {
    match event_name {
        "issue_comment" => handle_issue_comment(state, body).await,
        "pull_request_review_comment" => handle_pr_review_comment(state, body).await,
        "pull_request_review" => handle_pr_review(state, body).await,
        "pull_request" => handle_pull_request(state, body).await,
        "workflow_run" => handle_workflow_run(state, body).await,
        "check_run" => handle_check_run(state, body).await,
        "issues" => handle_issues(state, body).await,
        "ping" => {
            tracing::info!("received ping event");
            Ok(())
        }
        other => {
            tracing::debug!(event = other, "ignoring unhandled event");
            Ok(())
        }
    }
}

async fn handle_issue_comment(state: &AppState, body: &[u8]) -> Result<()> {
    let evt: IssueCommentEvent = serde_json::from_slice(body).context("parse issue_comment")?;
    if evt.action != "created" && evt.action != "edited" {
        return Ok(());
    }
    let Some(_) = &evt.issue.pull_request else {
        tracing::debug!("issue_comment on non-PR issue, skipping");
        return Ok(());
    };
    if !state.dedup.mark_seen("issue_comment", evt.comment.id)? {
        tracing::debug!(id = evt.comment.id, "dup issue_comment");
        return Ok(());
    }
    let key = PrKey::new(
        &evt.repository.owner.login,
        &evt.repository.name,
        evt.issue.number,
    );
    state.debouncer.push(key, BatchedEvent::Comment).await;
    Ok(())
}

async fn handle_pr_review_comment(state: &AppState, body: &[u8]) -> Result<()> {
    let evt: PrReviewCommentEvent =
        serde_json::from_slice(body).context("parse pr review_comment")?;
    if evt.action != "created" && evt.action != "edited" {
        return Ok(());
    }
    if !state.dedup.mark_seen("review_comment", evt.comment.id)? {
        return Ok(());
    }
    let key = PrKey::new(
        &evt.repository.owner.login,
        &evt.repository.name,
        evt.pull_request.number,
    );
    state.debouncer.push(key, BatchedEvent::Comment).await;
    Ok(())
}

async fn handle_pr_review(state: &AppState, body: &[u8]) -> Result<()> {
    let evt: PullRequestReviewEvent =
        serde_json::from_slice(body).context("parse pull_request_review")?;
    if evt.action != "submitted" {
        return Ok(());
    }
    if !state.dedup.mark_seen("review", evt.review.id)? {
        return Ok(());
    }
    let key = PrKey::new(
        &evt.repository.owner.login,
        &evt.repository.name,
        evt.pull_request.number,
    );
    state.debouncer.push(key, BatchedEvent::Review).await;
    Ok(())
}

async fn handle_pull_request(state: &AppState, body: &[u8]) -> Result<()> {
    let evt: PullRequestEvent = serde_json::from_slice(body).context("parse pull_request")?;
    match evt.action.as_str() {
        "opened" | "reopened" | "ready_for_review" => {
            let line = serde_json::json!({
                "ts": time::OffsetDateTime::now_utc()
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
                "kind": "pull_request",
                "action": evt.action,
                "repo": evt.repository.full_name,
                "pr": evt.pull_request.number,
                "title": evt.pull_request.title,
                "branch": evt.pull_request.head.r#ref,
                "url": evt.pull_request.html_url,
            });
            append_jsonl(&state.data_dir.join("gh-comments.jsonl"), &line).await?;
        }
        "synchronize" => {
            let key = PrKey::new(
                &evt.repository.owner.login,
                &evt.repository.name,
                evt.pull_request.number,
            );
            // fold into existing batch only — no new batch
            state.debouncer.push(key, BatchedEvent::Synchronize).await;
        }
        "closed" => {
            let line = serde_json::json!({
                "ts": time::OffsetDateTime::now_utc()
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
                "kind": "pull_request",
                "action": "closed",
                "repo": evt.repository.full_name,
                "pr": evt.pull_request.number,
                "title": evt.pull_request.title,
                "branch": evt.pull_request.head.r#ref,
                "url": evt.pull_request.html_url,
            });
            append_jsonl(&state.data_dir.join("gh-comments.jsonl"), &line).await?;
        }
        _ => {}
    }
    Ok(())
}

async fn handle_workflow_run(state: &AppState, body: &[u8]) -> Result<()> {
    let evt: WorkflowRunEvent = serde_json::from_slice(body).context("parse workflow_run")?;
    notify_workflow(&state.data_dir, &evt).await
}

async fn handle_check_run(state: &AppState, body: &[u8]) -> Result<()> {
    let evt: CheckRunEvent = serde_json::from_slice(body).context("parse check_run")?;
    notify_check_run(&state.data_dir, &evt).await
}

async fn handle_issues(state: &AppState, body: &[u8]) -> Result<()> {
    let evt: IssuesEvent = serde_json::from_slice(body).context("parse issues")?;
    if evt.action != "opened" && evt.action != "assigned" {
        return Ok(());
    }
    let line = serde_json::json!({
        "ts": time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        "kind": "issues",
        "action": evt.action,
        "repo": evt.repository.full_name,
        "number": evt.issue.number,
        "title": evt.issue.title,
        "author": evt.issue.user.login,
        "url": evt.issue.html_url,
    });
    append_jsonl(&state.data_dir.join("gh-comments.jsonl"), &line).await?;
    Ok(())
}
```

- [ ] **Step 3: Expose modules**

Edit `src/lib.rs`:
```rust
pub mod ci;
pub mod config;
pub mod debounce;
pub mod dedup;
pub mod dispatch;
pub mod events;
pub mod flush;
pub mod github;
pub mod hmac;
pub mod render;
pub mod state;
```

- [ ] **Step 4: Compile**

Run: `cargo build`
Expected: compiles.

- [ ] **Step 5: Commit**

```bash
git add tools/gh-webhook-server/
git commit -m "feat(gh-webhook-server): state + event dispatch"
```

---

## Task 12: Axum router and webhook handler

**Files:**
- Create: `tools/gh-webhook-server/src/router.rs`
- Modify: `tools/gh-webhook-server/src/main.rs`
- Modify: `tools/gh-webhook-server/src/lib.rs`

- [ ] **Step 1: Implement `src/router.rs`**

```rust
use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};

use crate::dispatch::handle;
use crate::hmac::validate_signature;
use crate::state::AppState;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/webhook", post(webhook))
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

async fn webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let Some(sig) = headers.get("X-Hub-Signature-256").and_then(|v| v.to_str().ok()) else {
        tracing::warn!("missing signature header");
        return (StatusCode::UNAUTHORIZED, "missing signature").into_response();
    };
    let Some(event) = headers.get("X-GitHub-Event").and_then(|v| v.to_str().ok()) else {
        tracing::warn!("missing X-GitHub-Event header");
        return (StatusCode::BAD_REQUEST, "missing event").into_response();
    };

    if let Err(e) = validate_signature(state.cfg.webhook_secret.as_bytes(), &body, sig) {
        tracing::warn!(error = %e, "signature validation failed");
        return (StatusCode::UNAUTHORIZED, "bad signature").into_response();
    }

    let event_name = event.to_string();
    let state_c = state.clone();
    tokio::spawn(async move {
        if let Err(err) = handle(&state_c, &event_name, &body).await {
            tracing::error!(event = event_name, error = ?err, "event handling failed");
        }
    });

    (StatusCode::ACCEPTED, "accepted").into_response()
}
```

- [ ] **Step 2: Expose and wire up**

Edit `src/lib.rs` — add `pub mod router;` (keep modules alphabetical).

Replace `src/main.rs`:

```rust
use anyhow::Result;
use gh_webhook_server::{config::Config, router::build_router, state::AppState};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("GH_WEBHOOK_LOG")
                .unwrap_or_else(|_| "gh_webhook_server=info,tower_http=info".into()),
        )
        .init();

    let cfg = Config::from_env()?;
    let bind = cfg.bind;
    let state = AppState::new(cfg)?;
    let app = build_router(state);

    let listener = TcpListener::bind(bind).await?;
    tracing::info!(%bind, "gh-webhook-server listening");
    axum::serve(listener, app).await?;
    Ok(())
}
```

- [ ] **Step 3: Compile + smoke run**

```bash
cargo build
GITHUB_WEBHOOK_SECRET=x GITHUB_TOKEN=x GITHUB_USER=x BIND=127.0.0.1:8787 DATA_DIR=/tmp/ghwh cargo run &
sleep 1
curl -sf http://127.0.0.1:8787/health
kill %1
```
Expected: `ok`, then server terminates cleanly.

- [ ] **Step 4: Commit**

```bash
git add tools/gh-webhook-server/
git commit -m "feat(gh-webhook-server): axum /health + /webhook route"
```

---

## Task 13: Router integration test with signed payload

**Files:**
- Create: `tools/gh-webhook-server/tests/integration_test.rs`

- [ ] **Step 1: Write the integration test**

```rust
use std::net::SocketAddr;
use std::time::Duration;

use gh_webhook_server::config::Config;
use gh_webhook_server::router::build_router;
use gh_webhook_server::state::AppState;
use hmac::{Hmac, Mac};
use reqwest::StatusCode;
use sha2::Sha256;
use tempfile::tempdir;
use tokio::net::TcpListener;

fn sign(secret: &[u8], body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

async fn spawn_server(dir: &std::path::Path, secret: &str) -> SocketAddr {
    let cfg = Config {
        webhook_secret: secret.into(),
        github_token: "dummy".into(),
        github_user: "jmagar".into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        data_dir: dir.to_path_buf(),
        debounce_secs: 1,
    };
    let state = AppState::new(cfg).unwrap();
    let app = build_router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    addr
}

#[tokio::test]
async fn health_endpoint_ok() {
    let dir = tempdir().unwrap();
    let addr = spawn_server(dir.path(), "s").await;
    let body = reqwest::get(format!("http://{addr}/health"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn rejects_unsigned_webhook() {
    let dir = tempdir().unwrap();
    let addr = spawn_server(dir.path(), "s").await;
    let res = reqwest::Client::new()
        .post(format!("http://{addr}/webhook"))
        .header("X-GitHub-Event", "ping")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn accepts_signed_ping() {
    let dir = tempdir().unwrap();
    let secret = "s";
    let addr = spawn_server(dir.path(), secret).await;
    let body = b"{\"zen\":\"test\"}";
    let res = reqwest::Client::new()
        .post(format!("http://{addr}/webhook"))
        .header("X-GitHub-Event", "ping")
        .header("X-Hub-Signature-256", sign(secret.as_bytes(), body))
        .body(body.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn pull_request_opened_writes_jsonl() {
    let dir = tempdir().unwrap();
    let secret = "s";
    let addr = spawn_server(dir.path(), secret).await;
    let body = include_bytes!("fixtures/pull_request_opened.json");
    let res = reqwest::Client::new()
        .post(format!("http://{addr}/webhook"))
        .header("X-GitHub-Event", "pull_request")
        .header("X-Hub-Signature-256", sign(secret.as_bytes(), body))
        .body(body.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);

    // Handler is spawned as a task — wait briefly
    tokio::time::sleep(Duration::from_millis(200)).await;

    let jsonl = dir.path().join("gh-comments.jsonl");
    let contents = tokio::fs::read_to_string(&jsonl).await.unwrap();
    assert!(contents.contains("\"kind\":\"pull_request\""));
    assert!(contents.contains("\"action\":\"opened\""));
    assert!(contents.contains("\"pr\":99"));
}
```

- [ ] **Step 2: Run — expect pass**

Run: `cargo test --test integration_test`
Expected: 4 passed.

- [ ] **Step 3: Commit**

```bash
git add tools/gh-webhook-server/
git commit -m "test(gh-webhook-server): integration tests for /health + /webhook"
```

---

## Task 14: Register webhook on a single repo

**Files:**
- Create: `tools/gh-webhook-server/scripts/register.sh`

- [ ] **Step 1: Write `scripts/register.sh`**

```bash
#!/usr/bin/env bash
# Register the gh-webhook-server webhook on a single repo.
# Usage: ./register.sh <owner/repo> <public-url>
# Requires: gh CLI authenticated, GITHUB_WEBHOOK_SECRET in env.
set -euo pipefail

REPO="${1:?usage: register.sh <owner/repo> <public-url>}"
URL="${2:?usage: register.sh <owner/repo> <public-url>}"
SECRET="${GITHUB_WEBHOOK_SECRET:?set GITHUB_WEBHOOK_SECRET}"

EVENTS='["issue_comment","pull_request_review_comment","pull_request_review","pull_request","workflow_run","check_run","issues"]'

# Check if a hook already points at our URL
existing=$(gh api "repos/${REPO}/hooks" --jq ".[] | select(.config.url == \"${URL}\") | .id" || true)

if [[ -n "${existing}" ]]; then
  echo "updating existing hook ${existing} on ${REPO}"
  gh api -X PATCH "repos/${REPO}/hooks/${existing}" \
    -f "config[url]=${URL}" \
    -f "config[content_type]=json" \
    -f "config[secret]=${SECRET}" \
    -F "active=true" \
    --input - <<EOF
{"events": ${EVENTS}}
EOF
else
  echo "creating hook on ${REPO}"
  gh api -X POST "repos/${REPO}/hooks" \
    -f "name=web" \
    -F "active=true" \
    -f "config[url]=${URL}" \
    -f "config[content_type]=json" \
    -f "config[secret]=${SECRET}" \
    --input - <<EOF
{"events": ${EVENTS}}
EOF
fi
```

- [ ] **Step 2: Make it executable**

Run: `chmod +x tools/gh-webhook-server/scripts/register.sh`

- [ ] **Step 3: Verify syntax**

Run: `bash -n tools/gh-webhook-server/scripts/register.sh`
Expected: no output (valid shell).

- [ ] **Step 4: Commit**

```bash
git add tools/gh-webhook-server/scripts/register.sh
git commit -m "feat(gh-webhook-server): per-repo webhook registration script"
```

---

## Task 15: Register webhooks across all user repos

**Files:**
- Create: `tools/gh-webhook-server/scripts/register-all.sh`

- [ ] **Step 1: Write `scripts/register-all.sh`**

```bash
#!/usr/bin/env bash
# Register the webhook on every non-archived, non-fork repo owned by $GITHUB_USER.
# Usage: ./register-all.sh <public-url>
set -euo pipefail

URL="${1:?usage: register-all.sh <public-url>}"
USER="${GITHUB_USER:?set GITHUB_USER}"

here="$(cd "$(dirname "$0")" && pwd)"

mapfile -t repos < <(
  gh repo list "${USER}" --limit 500 --no-archived --source \
    --json nameWithOwner --jq '.[].nameWithOwner'
)

echo "registering on ${#repos[@]} repos"
for repo in "${repos[@]}"; do
  echo "---- ${repo}"
  if ! "${here}/register.sh" "${repo}" "${URL}"; then
    echo "WARN: failed to register ${repo}" >&2
  fi
done
```

- [ ] **Step 2: Make executable**

Run: `chmod +x tools/gh-webhook-server/scripts/register-all.sh`

- [ ] **Step 3: Validate syntax**

Run: `bash -n tools/gh-webhook-server/scripts/register-all.sh`
Expected: no output.

- [ ] **Step 4: Commit**

```bash
git add tools/gh-webhook-server/scripts/register-all.sh
git commit -m "feat(gh-webhook-server): bulk webhook registration across user repos"
```

---

## Task 16: systemd user unit

**Files:**
- Create: `tools/gh-webhook-server/systemd/gh-webhook-server.service`

- [ ] **Step 1: Write the unit file**

```ini
[Unit]
Description=GitHub webhook server for gh-address-comments
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
EnvironmentFile=%h/.config/gh-webhook-server/env
ExecStart=%h/.local/bin/gh-webhook-server
Restart=on-failure
RestartSec=5s
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=default.target
```

- [ ] **Step 2: Verify syntax (systemd-analyze verify if available)**

Run: `systemd-analyze --user verify tools/gh-webhook-server/systemd/gh-webhook-server.service 2>&1 | head -20 || echo "skipped (systemd-analyze not available)"`
Expected: no errors, or skip message.

- [ ] **Step 3: Commit**

```bash
git add tools/gh-webhook-server/systemd/
git commit -m "feat(gh-webhook-server): systemd user unit"
```

---

## Task 17: README with Tailscale Funnel setup

**Files:**
- Create: `tools/gh-webhook-server/README.md`

- [ ] **Step 1: Write the README**

```markdown
# gh-webhook-server

Rust webhook receiver that coalesces GitHub PR/comment/CI activity into short
notification lines consumed by Claude Code monitors.

## What it does

- Receives GitHub webhooks on `/webhook`.
- Validates `X-Hub-Signature-256` (HMAC-SHA256) against `GITHUB_WEBHOOK_SECRET`.
- For PR comment/review events: debounces per-PR for 30s, then fetches the full
  thread set via the GitHub REST API and writes a Markdown digest to
  `$DATA_DIR/gh-comments/<owner>_<repo>_<pr>/latest.md`.
- Appends one JSONL line per batch to `$DATA_DIR/gh-comments.jsonl`.
- Writes failed-CI notifications to `$DATA_DIR/gh-ci.jsonl` (no debounce).
- Dedups by comment/review ID in SQLite so webhook redeliveries do not
  double-notify.

## Handled events

| Event | Action | Result |
|-------|--------|--------|
| `issue_comment` | created / edited | add to PR batch |
| `pull_request_review_comment` | created / edited | add to PR batch |
| `pull_request_review` | submitted | add to PR batch |
| `pull_request` | opened / reopened / ready_for_review | immediate jsonl line |
| `pull_request` | synchronize | fold into existing batch |
| `pull_request` | closed | immediate jsonl line |
| `workflow_run` | completed (failure) | immediate gh-ci.jsonl line |
| `check_run` | completed (failure) | immediate gh-ci.jsonl line |
| `issues` | opened / assigned | immediate jsonl line |

## Install

```bash
cargo install --path tools/gh-webhook-server --root ~/.local
```

## Configure

Copy `.env.example` to `~/.config/gh-webhook-server/env` and edit:

```env
GITHUB_WEBHOOK_SECRET=<openssl rand -hex 32>
GITHUB_TOKEN=<gh auth token>
GITHUB_USER=jmagar
BIND=127.0.0.1:8787
DATA_DIR=/home/jmagar/.claude
DEBOUNCE_SECS=30
```

## Run

```bash
systemctl --user enable --now gh-webhook-server
systemctl --user status gh-webhook-server
journalctl --user -u gh-webhook-server -f
```

## Expose to GitHub

### Tailscale Funnel (recommended)

```bash
tailscale funnel --bg --https=443 --set-path=/webhook http://127.0.0.1:8787/webhook
# note the public URL printed, e.g. https://mymachine.tailXXXX.ts.net/webhook
```

### Cloudflare Tunnel (alternative)

```bash
cloudflared tunnel run --url http://127.0.0.1:8787
```

## Register webhooks

Single repo:
```bash
export GITHUB_WEBHOOK_SECRET=...
./scripts/register.sh jmagar/lab https://mymachine.tailXXXX.ts.net/webhook
```

All repos:
```bash
export GITHUB_WEBHOOK_SECRET=... GITHUB_USER=jmagar
./scripts/register-all.sh https://mymachine.tailXXXX.ts.net/webhook
```

## Verify end-to-end

1. Ship a test comment on any PR.
2. Within 30 seconds:
   - `tail -f ~/.claude/gh-comments.jsonl` shows a new line.
   - `~/.claude/gh-comments/<owner>_<repo>_<pr>/latest.md` is updated.
3. `journalctl --user -u gh-webhook-server -f` shows `flush complete`.

## Troubleshooting

- **401 bad signature** — `GITHUB_WEBHOOK_SECRET` mismatch between env and the hook secret on GitHub.
- **No flush firing** — check that `DEBOUNCE_SECS` elapsed and that GitHub REST API is reachable (`GITHUB_TOKEN` valid).
- **Duplicated notifications** — dedup DB may have been wiped; redeliveries will re-notify once.
```

- [ ] **Step 2: Commit**

```bash
git add tools/gh-webhook-server/README.md
git commit -m "docs(gh-webhook-server): README with Tailscale Funnel setup"
```

---

## Task 18: Monitor wrapper scripts

**Files:**
- Create: `skills/gh-address-comments/monitors/gh-comments.monitor.sh`
- Create: `skills/gh-address-comments/monitors/gh-ci.monitor.sh`
- Create: `skills/gh-address-comments/monitors/README.md`

The wrappers tail the JSONL streams and print one short, human-readable line per entry. Claude Code plugin monitors treat each stdout line as a notification.

- [ ] **Step 1: Write `gh-comments.monitor.sh`**

```bash
#!/usr/bin/env bash
# Claude Code plugin monitor: streams gh-comments.jsonl as short notifications.
# Called by monitors/monitors.json — each line printed becomes a notification.
set -euo pipefail

JSONL="${GH_COMMENTS_JSONL:-$HOME/.claude/gh-comments.jsonl}"

# Create if missing so tail does not fail
mkdir -p "$(dirname "$JSONL")"
touch "$JSONL"

exec tail -n 0 -F "$JSONL" 2>/dev/null | jq -r '
  if .kind == "pull_request" then
    if .action == "opened" or .action == "reopened" or .action == "ready_for_review" then
      "[PR \(.action|ascii_upcase)] \(.repo) #\(.pr) (\(.branch)) — \(.title) → \(.url)"
    elif .action == "closed" then
      "[PR CLOSED] \(.repo) #\(.pr) (\(.branch)) — \(.title)"
    else empty end
  elif .kind == "issues" then
    "[ISSUE \(.action|ascii_upcase)] \(.repo) #\(.number) — \(.title) (@\(.author)) → \(.url)"
  elif .pr then
    # default: comment batch
    "[\(.new_events) new] \(.repo) #\(.pr) (\(.branch)) — \(.review_threads) threads, \(.conversation) conv — view: \(.path)"
  else empty end
'
```

- [ ] **Step 2: Write `gh-ci.monitor.sh`**

```bash
#!/usr/bin/env bash
# Claude Code plugin monitor: streams gh-ci.jsonl as short CI-fail notifications.
set -euo pipefail

JSONL="${GH_CI_JSONL:-$HOME/.claude/gh-ci.jsonl}"
mkdir -p "$(dirname "$JSONL")"
touch "$JSONL"

exec tail -n 0 -F "$JSONL" 2>/dev/null | jq -r '
  if .pr then
    "[CI \(.conclusion|ascii_upcase)] \(.repo) #\(.pr) — \(.name) — \(.url)"
  else
    "[CI \(.conclusion|ascii_upcase)] \(.repo) (\(.branch)) — \(.name) — \(.url)"
  end
'
```

- [ ] **Step 3: Make executable**

```bash
chmod +x skills/gh-address-comments/monitors/gh-comments.monitor.sh \
         skills/gh-address-comments/monitors/gh-ci.monitor.sh
```

- [ ] **Step 4: Write `skills/gh-address-comments/monitors/README.md`**

```markdown
# gh-address-comments monitors

Live notifications from the `gh-webhook-server` streams.

## Files

- `gh-comments.monitor.sh` — tails `~/.claude/gh-comments.jsonl`; one short line per PR batch, per new PR, per closed PR, per new/assigned issue.
- `gh-ci.monitor.sh` — tails `~/.claude/gh-ci.jsonl`; one short line per failed workflow / check run.

## Wiring

The repo-root `monitors/monitors.json` registers both:

```json
{ "name": "gh-comments-monitor",
  "command": "skills/gh-address-comments/monitors/gh-comments.monitor.sh",
  "description": "New PR/issue comment batches and lifecycle events" }
```

Claude Code runs the `command`, treats each stdout line as a notification, and pings the active session. Lines include a path to `latest.md`; Claude reads that on demand.

## Producing the streams

See `tools/gh-webhook-server/README.md` — that server is the producer of both JSONL files. Without it running, the monitors stay idle.
```

- [ ] **Step 5: Verify shell syntax**

```bash
bash -n skills/gh-address-comments/monitors/gh-comments.monitor.sh
bash -n skills/gh-address-comments/monitors/gh-ci.monitor.sh
```
Expected: no output.

- [ ] **Step 6: Commit**

```bash
git add skills/gh-address-comments/monitors/
git commit -m "feat(gh-address-comments): monitor wrappers for jsonl streams"
```

---

## Task 19: Register monitors in `monitors/monitors.json`

**Files:**
- Modify: `monitors/monitors.json`

- [ ] **Step 1: Replace `monitors/monitors.json`**

```json
[
  {
    "name": "deploy-host-monitor",
    "command": "lab deploy monitor ${user_config.deploy_targets} --interval 60",
    "description": "SSH host reachability — notifies when a deployed host goes online or offline"
  },
  {
    "name": "gh-comments-monitor",
    "command": "skills/gh-address-comments/monitors/gh-comments.monitor.sh",
    "description": "New PR/issue comment batches and PR lifecycle events from gh-webhook-server"
  },
  {
    "name": "gh-ci-monitor",
    "command": "skills/gh-address-comments/monitors/gh-ci.monitor.sh",
    "description": "Failed workflow/check runs from gh-webhook-server"
  }
]
```

- [ ] **Step 2: Validate JSON**

Run: `cat monitors/monitors.json | jq .`
Expected: pretty-printed JSON, exit 0.

- [ ] **Step 3: Commit**

```bash
git add monitors/monitors.json
git commit -m "feat(monitors): register gh-comments + gh-ci monitors"
```

---

## Task 20: Extend SKILL.md with live-notifications section

**Files:**
- Modify: `skills/gh-address-comments/SKILL.md`

- [ ] **Step 1: Add a new section**

Insert the following section after the existing "Available CLI Tools" block, before "Workflow":

```markdown
## Live notifications (optional)

When the companion `tools/gh-webhook-server` is running, Claude gets short
notifications the moment GitHub sees activity on your PRs — no polling.

Each notification is one line pointing at a pre-rendered Markdown digest:

```
[4 new] jmagar/lab #123 (fix/auth) — 7 threads, 2 conv — view: /home/jmagar/.claude/gh-comments/jmagar_lab_123/latest.md
```

When you see one of these lines:

1. `Read` the `latest.md` path to see the grouped threads.
2. Run `gh-fetch-comments --pr <N> -o /tmp/pr.json` to create/update beads.
3. Proceed with the normal Workflow below.

For CI failures, lines come from the `gh-ci-monitor` and look like:

```
[CI FAILURE] jmagar/lab #123 — test (ubuntu) — https://github.com/jmagar/lab/actions/runs/555
```

Set up:
- See `tools/gh-webhook-server/README.md` for install + Tailscale Funnel.
- See `skills/gh-address-comments/monitors/README.md` for the stream wrappers.

If the webhook server is not running, this section is dormant — the rest of the skill still works via manual `gh-fetch-comments` calls.
```

- [ ] **Step 2: Verify markdown renders sensibly**

Run: `head -120 skills/gh-address-comments/SKILL.md`
Expected: the new section appears between "Available CLI Tools" and "Workflow".

- [ ] **Step 3: Commit**

```bash
git add skills/gh-address-comments/SKILL.md
git commit -m "docs(gh-address-comments): document live notifications via webhook server"
```

---

## Task 21: End-to-end smoke verification

**Files:**
- None (runtime verification only)

- [ ] **Step 1: Install the binary**

```bash
cargo install --path tools/gh-webhook-server --root ~/.local --force
```

- [ ] **Step 2: Create config**

```bash
mkdir -p ~/.config/gh-webhook-server
cat > ~/.config/gh-webhook-server/env <<EOF
GITHUB_WEBHOOK_SECRET=$(openssl rand -hex 32)
GITHUB_TOKEN=$(gh auth token)
GITHUB_USER=jmagar
BIND=127.0.0.1:8787
DATA_DIR=$HOME/.claude
DEBOUNCE_SECS=30
EOF
chmod 600 ~/.config/gh-webhook-server/env
```

- [ ] **Step 3: Install and start systemd unit**

```bash
mkdir -p ~/.config/systemd/user
cp tools/gh-webhook-server/systemd/gh-webhook-server.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now gh-webhook-server
systemctl --user status gh-webhook-server --no-pager
```
Expected: `active (running)`.

- [ ] **Step 4: Health check**

```bash
curl -sf http://127.0.0.1:8787/health
```
Expected: `ok`.

- [ ] **Step 5: Start Tailscale Funnel**

```bash
tailscale funnel --bg --https=443 --set-path=/webhook http://127.0.0.1:8787/webhook
tailscale funnel status
```
Expected: shows public URL mapped to `/webhook`.

- [ ] **Step 6: Register a single repo as a smoke test**

```bash
export GITHUB_WEBHOOK_SECRET=$(grep GITHUB_WEBHOOK_SECRET ~/.config/gh-webhook-server/env | cut -d= -f2)
PUBLIC_URL="https://$(tailscale status --json | jq -r '.Self.DNSName' | sed 's/\.$//')/webhook"
tools/gh-webhook-server/scripts/register.sh jmagar/lab "$PUBLIC_URL"
```
Expected: "creating hook on jmagar/lab" or "updating existing hook …".

- [ ] **Step 7: Trigger a GitHub "ping" from the hook settings**

```bash
HOOK_ID=$(gh api repos/jmagar/lab/hooks --jq ".[] | select(.config.url==\"$PUBLIC_URL\") | .id")
gh api -X POST "repos/jmagar/lab/hooks/${HOOK_ID}/pings"
```

- [ ] **Step 8: Verify in logs**

```bash
journalctl --user -u gh-webhook-server -n 20 --no-pager | grep "received ping"
```
Expected: at least one matching line.

- [ ] **Step 9: Post a test comment on a real PR, wait 35s, verify**

```bash
tail -f ~/.claude/gh-comments.jsonl &
sleep 40 && kill %1
ls -la ~/.claude/gh-comments/
```
Expected: a new jsonl line and a `<owner>_<repo>_<pr>/latest.md` file appear within ~35s of posting the comment.

- [ ] **Step 10: Register all repos**

```bash
export GITHUB_USER=jmagar
tools/gh-webhook-server/scripts/register-all.sh "$PUBLIC_URL"
```

- [ ] **Step 11: Final commit if any runtime fixes were needed**

```bash
git status
# if changes: git add … && git commit -m "fix(gh-webhook-server): post-smoke-test fixes"
```
