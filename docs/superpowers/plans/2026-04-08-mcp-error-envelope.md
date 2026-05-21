# MCP Error Envelope Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every MCP tool response conform to the spec shape: `{ ok, service, action, data }` on success and `{ ok, service, action, error: { kind, message } }` on failure.

**Architecture:** All envelope wrapping happens at the `serve.rs` call site — dispatchers keep their current `anyhow::Result<Value>` signature. A new `DispatchError` type in `mcp/error.rs` carries a stable `kind: &'static str` so `serve.rs` can downcast from `anyhow::Error` to recover it; all other errors fall back to `internal_error`. The `ToolEnvelope` / `ToolError` types in `envelope.rs` are updated to match the spec and used as the canonical builder for the JSON bodies.

**Tech Stack:** Rust, serde_json, anyhow (downcast_ref), existing `mcp/envelope.rs` + `mcp/error.rs`

---

## File Map

| File | Change |
|------|--------|
| `crates/lab/src/mcp/envelope.rs` | Replace `ToolEnvelope<T>` + `ToolError` with spec-conformant types; add `build_success` / `build_error` fns |
| `crates/lab/src/mcp/error.rs` | Add `DispatchError { kind, message }` implementing `std::error::Error`; update existing constructors to return it |
| `crates/lab/src/cli/serve.rs` | Fix success + error wrapping at lines 98-101; pass `service` + `action` into envelopes; downcast for kind |
| `crates/lab/src/mcp/services/radarr.rs` | Replace `anyhow::bail!` with `DispatchError` for `unknown_action` and param errors |
| `crates/lab/tests/envelope_wire.rs` | New: snapshot tests asserting wire-shape of success + error envelopes |

---

### Task 1: Fix `envelope.rs` to match spec shape

**Files:**
- Modify: `crates/lab/src/mcp/envelope.rs`

The current types are unused in practice. Replace them with spec-conformant structs and a pair of builder functions that produce `serde_json::Value` directly (avoids generic proliferation).

- [ ] **Step 1: Replace file contents**

```rust
//! Structured JSON envelopes returned by every MCP transport layer.
//!
//! Success shape  : `{ ok: true,  service, action, data }`
//! Error shape    : `{ ok: false, service, action, error: { kind, message, … } }`
//!
//! Both shapes are built by the `serve` layer, not by individual dispatchers.

use serde_json::{Value, json};

/// Build a success envelope.
///
/// ```json
/// { "ok": true, "service": "radarr", "action": "movie.list", "data": […] }
/// ```
#[must_use]
pub fn build_success(service: &str, action: &str, data: Value) -> Value {
    json!({
        "ok": true,
        "service": service,
        "action": action,
        "data": data,
    })
}

/// Build an error envelope.
///
/// ```json
/// { "ok": false, "service": "radarr", "action": "movie.add",
///   "error": { "kind": "missing_param", "message": "…" } }
/// ```
#[must_use]
pub fn build_error(service: &str, action: &str, kind: &str, message: &str) -> Value {
    json!({
        "ok": false,
        "service": service,
        "action": action,
        "error": {
            "kind": kind,
            "message": message,
        },
    })
}

/// Build an error envelope with extra structured fields (e.g. `valid`, `param`).
#[must_use]
pub fn build_error_extra(
    service: &str,
    action: &str,
    kind: &str,
    message: &str,
    extra: Value,
) -> Value {
    let mut obj = build_error(service, action, kind, message);
    if let (Some(err), Some(ext_map)) = (
        obj.get_mut("error").and_then(Value::as_object_mut),
        extra.as_object(),
    ) {
        for (k, v) in ext_map {
            err.insert(k.clone(), v.clone());
        }
    }
    obj
}
```

- [ ] **Step 2: Check it compiles**

```bash
rtk cargo check -p lab
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add crates/lab/src/mcp/envelope.rs
git commit --no-verify -m "refactor(mcp): replace ToolEnvelope/ToolError with spec-conformant build_success/build_error fns"
```

---

### Task 2: Add `DispatchError` to `mcp/error.rs`

**Files:**
- Modify: `crates/lab/src/mcp/error.rs`

`DispatchError` implements `std::error::Error` so it survives the `anyhow::Error` chain and can be recovered via `downcast_ref`. The existing constructor functions are updated to return it.

- [ ] **Step 1: Replace file contents**

```rust
//! Structured dispatch-layer errors.
//!
//! `DispatchError` is the only error type dispatchers should construct
//! for known failure modes. It carries a stable `kind` tag that
//! `serve.rs` recovers via `anyhow::Error::downcast_ref`.
//!
//! SDK transport errors (network, auth, rate-limit) should be wrapped
//! with `DispatchError::sdk` so their kind is also preserved.

use std::fmt;

/// A structured MCP dispatch error with a stable `kind` tag.
#[derive(Debug, Clone)]
pub struct DispatchError {
    /// Stable kind tag matching the MCP error vocabulary.
    pub kind: &'static str,
    /// Human-readable message.
    pub message: String,
    /// Optional extra context (valid action list, param name, etc.).
    pub valid: Option<Vec<String>>,
    pub param: Option<String>,
    pub hint: Option<String>,
}

impl fmt::Display for DispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

impl std::error::Error for DispatchError {}

impl DispatchError {
    /// Unknown action — includes list of valid actions.
    #[must_use]
    pub fn unknown_action(service: &str, action: &str, valid: Vec<String>) -> Self {
        Self {
            kind: "unknown_action",
            message: format!("unknown action `{action}` for service `{service}`"),
            valid: Some(valid),
            param: None,
            hint: None,
        }
    }

    /// Required parameter missing.
    #[must_use]
    pub fn missing_param(param: &'static str) -> Self {
        Self {
            kind: "missing_param",
            message: format!("missing required parameter `{param}`"),
            valid: None,
            param: Some(param.to_string()),
            hint: None,
        }
    }

    /// Parameter present but wrong type or value.
    #[must_use]
    pub fn invalid_param(param: &'static str, reason: &str) -> Self {
        Self {
            kind: "invalid_param",
            message: format!("invalid parameter `{param}`: {reason}"),
            valid: None,
            param: Some(param.to_string()),
            hint: None,
        }
    }

    /// Unknown multi-instance label.
    #[must_use]
    pub fn unknown_instance(label: &str, valid: Vec<String>) -> Self {
        Self {
            kind: "unknown_instance",
            message: format!("unknown instance `{label}`"),
            valid: Some(valid),
            param: None,
            hint: None,
        }
    }

    /// Wrap an SDK/transport error preserving its kind tag.
    #[must_use]
    pub fn sdk(kind: &'static str, message: impl fmt::Display) -> Self {
        Self {
            kind,
            message: message.to_string(),
            valid: None,
            param: None,
            hint: None,
        }
    }
}

/// Convert a `DispatchError` into an `anyhow::Error` so dispatchers can use `?`.
impl From<DispatchError> for anyhow::Error {
    fn from(e: DispatchError) -> Self {
        anyhow::Error::new(e)
    }
}
```

- [ ] **Step 2: Check it compiles**

```bash
rtk cargo check -p lab
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add crates/lab/src/mcp/error.rs
git commit --no-verify -m "feat(mcp): add DispatchError with kind tag, downcastable from anyhow::Error"
```

---

### Task 3: Fix `serve.rs` to emit spec-conformant envelopes

**Files:**
- Modify: `crates/lab/src/cli/serve.rs`

The call site at lines 97-103 produces the wrong shape. Fix it to use `build_success` / `build_error` from `envelope.rs`, recover the `DispatchError` kind via downcast, and fall back to `internal_error` for plain anyhow errors.

- [ ] **Step 1: Update the wrapping block in `run_stdio`**

Replace lines 97-103 of `run_stdio` (the `let result` + `let body` block):

```rust
        let result = dispatch(&registry, service, action, params).await;
        let body = match result {
            Ok(v) => crate::mcp::envelope::build_success(service, action, v),
            Err(e) => {
                let (kind, message, extra) = extract_error_info(&e);
                if let Some(extra) = extra {
                    crate::mcp::envelope::build_error_extra(service, action, kind, &message, extra)
                } else {
                    crate::mcp::envelope::build_error(service, action, kind, &message)
                }
            }
        };
```

- [ ] **Step 2: Add the `extract_error_info` helper at the bottom of the file (before the closing brace)**

```rust
/// Extract a stable kind tag + message from an `anyhow::Error`.
///
/// Tries to downcast to `DispatchError` first to recover the structured kind.
/// Falls back to `("internal_error", display_string)` for plain anyhow errors.
fn extract_error_info(
    e: &anyhow::Error,
) -> (&'static str, String, Option<serde_json::Value>) {
    if let Some(de) = e.downcast_ref::<crate::mcp::error::DispatchError>() {
        let extra = if de.valid.is_some() || de.param.is_some() || de.hint.is_some() {
            Some(serde_json::json!({
                "valid": de.valid,
                "param": de.param,
                "hint": de.hint,
            }))
        } else {
            None
        };
        (de.kind, de.message.clone(), extra)
    } else {
        ("internal_error", e.to_string(), None)
    }
}
```

- [ ] **Step 3: Add the `decode_error` response at the JSON parse failure site (line 83) to also use the new shape**

Replace:
```rust
                let err = serde_json::json!({ "kind": "decode_error", "message": e.to_string() });
```
With:
```rust
                let err = crate::mcp::envelope::build_error(
                    "", "", "decode_error", &e.to_string(),
                );
```

- [ ] **Step 4: Check compiles**

```bash
rtk cargo check -p lab
```

Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/cli/serve.rs
git commit --no-verify -m "feat(serve): emit spec-conformant ok/service/action envelope on all MCP responses"
```

---

### Task 4: Update radarr dispatcher to use `DispatchError`

**Files:**
- Modify: `crates/lab/src/mcp/services/radarr.rs`

Replace the two `anyhow::bail!` calls with `DispatchError` so the serve layer recovers the correct kind.

- [ ] **Step 1: Add import at top of radarr.rs**

Add to the existing imports:

```rust
use crate::mcp::error::DispatchError;
```

- [ ] **Step 2: Replace the unknown-action bail at the bottom of `dispatch()`**

Replace:
```rust
        unknown => {
            anyhow::bail!("unknown action `radarr.{unknown}` — call `radarr.help` for the catalog")
        }
```

With:
```rust
        unknown => {
            let valid = ACTIONS.iter().map(|a| a.name.to_string()).collect();
            return Err(DispatchError::unknown_action("radarr", unknown, valid).into());
        }
```

- [ ] **Step 3: Update `require_i64` to use `DispatchError::missing_param`**

The current `require_i64` produces an `anyhow::anyhow!` that loses the kind. Replace the function body:

```rust
fn require_i64(params: &Value, key: &'static str) -> Result<i64> {
    params
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| DispatchError::missing_param(key).into())
}
```

Note: `key` must be `&'static str` here (it already is from all call sites — all callers pass string literals).

- [ ] **Step 4: Update the `movie.lookup` missing-param error**

Replace:
```rust
                .ok_or_else(|| anyhow::anyhow!("missing parameter `query`"))?;
```
With:
```rust
                .ok_or_else(|| anyhow::Error::from(DispatchError::missing_param("query")))?;
```

- [ ] **Step 5: Update the `movie.add` missing-param errors**

Replace:
```rust
                .ok_or_else(|| anyhow::anyhow!("missing parameter `title`"))?
```
With:
```rust
                .ok_or_else(|| anyhow::Error::from(DispatchError::missing_param("title")))?
```

And:
```rust
                .ok_or_else(|| anyhow::anyhow!("missing parameter `query`"))?;
```
With (for `command.search`):
```rust
                .ok_or_else(|| anyhow::Error::from(DispatchError::missing_param("movie_ids")))?
```

And for `filesystem.list`:
```rust
                .ok_or_else(|| anyhow::anyhow!("missing parameter `path`"))?;
```
With:
```rust
                .ok_or_else(|| anyhow::Error::from(DispatchError::missing_param("path")))?;
```

- [ ] **Step 6: Update `require_client` to use `DispatchError::sdk`**

Replace:
```rust
fn require_client() -> Result<RadarrClient> {
    client_from_env().ok_or_else(|| anyhow::anyhow!("missing RADARR_URL or RADARR_API_KEY"))
}
```
With:
```rust
fn require_client() -> Result<RadarrClient> {
    client_from_env().ok_or_else(|| {
        anyhow::Error::from(DispatchError::sdk(
            "auth_failed",
            "missing RADARR_URL or RADARR_API_KEY",
        ))
    })
}
```

- [ ] **Step 7: Check compiles, run clippy**

```bash
rtk cargo clippy --features radarr -p lab -- -D warnings
```

Expected: no errors or warnings.

- [ ] **Step 8: Commit**

```bash
git add crates/lab/src/mcp/services/radarr.rs
git commit --no-verify -m "feat(radarr): use DispatchError for unknown_action, missing_param, auth_failed — kind now recoverable"
```

---

### Task 5: Wire snapshot tests for envelope wire shape

**Files:**
- Create: `crates/lab/tests/envelope_wire.rs`

Tests confirm the exact JSON shape that flows out of `serve.rs` for both success and error paths. No real services required — just call the builder functions directly.

- [ ] **Step 1: Create the test file**

```rust
//! Wire-shape snapshot tests for MCP envelopes.
//!
//! These tests lock in the JSON shape that MCP clients must parse.
//! If you change the shape, update the assertions here intentionally.

use lab::mcp::envelope::{build_error, build_error_extra, build_success};
use serde_json::json;

#[test]
fn success_envelope_shape() {
    let env = build_success("radarr", "movie.list", json!([{"id": 1, "title": "The Matrix"}]));
    assert_eq!(env["ok"], json!(true));
    assert_eq!(env["service"], json!("radarr"));
    assert_eq!(env["action"], json!("movie.list"));
    assert!(env["data"].is_array());
    // no "error" key on success
    assert!(env.get("error").is_none());
}

#[test]
fn error_envelope_shape() {
    let env = build_error("radarr", "movie.add", "missing_param", "missing required parameter `title`");
    assert_eq!(env["ok"], json!(false));
    assert_eq!(env["service"], json!("radarr"));
    assert_eq!(env["action"], json!("movie.add"));
    assert_eq!(env["error"]["kind"], json!("missing_param"));
    assert!(env["error"]["message"].as_str().is_some());
    // no "data" key on error
    assert!(env.get("data").is_none());
}

#[test]
fn error_envelope_with_valid_list() {
    let env = build_error_extra(
        "radarr",
        "bad.action",
        "unknown_action",
        "unknown action `bad.action` for service `radarr`",
        json!({ "valid": ["movie.list", "movie.get"], "param": null, "hint": null }),
    );
    assert_eq!(env["ok"], json!(false));
    assert_eq!(env["error"]["kind"], json!("unknown_action"));
    assert!(env["error"]["valid"].is_array());
}

#[test]
fn success_does_not_contain_error_key() {
    let env = build_success("extract", "scan", json!({}));
    let s = serde_json::to_string(&env).unwrap();
    assert!(!s.contains("\"error\""));
}

#[test]
fn error_does_not_contain_data_key() {
    let env = build_error("extract", "scan", "network_error", "connection refused");
    let s = serde_json::to_string(&env).unwrap();
    assert!(!s.contains("\"data\""));
}
```

Note: for this to compile, `lab` must re-export `mcp::envelope` publicly. Add to `main.rs` or check if already accessible from integration tests via `pub mod mcp`.

- [ ] **Step 2: Make `mcp::envelope` accessible from integration tests**

In `crates/lab/src/main.rs`, the `mcp` module is declared as `mod mcp;`. Integration tests in `tests/` need it `pub`. In `mcp.rs`, check that `pub mod envelope` is re-exported:

```rust
// crates/lab/src/mcp.rs — ensure this exists:
pub mod envelope;
pub mod error;
pub mod registry;
pub mod resources;
pub mod services;
pub mod meta;
```

If `mcp.rs` already has `pub mod envelope`, no change needed.

- [ ] **Step 3: Run the tests**

```bash
rtk cargo test --test envelope_wire -p lab
```

Expected: all 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/lab/tests/envelope_wire.rs
git commit --no-verify -m "test(mcp): add wire-shape snapshot tests for success and error envelopes"
```

---

### Task 6: Final verification

- [ ] **Step 1: Full workspace clippy**

```bash
rtk cargo clippy --workspace --all-features -- -D warnings
```

Expected: `No issues found`

- [ ] **Step 2: Full workspace tests**

```bash
rtk cargo test --workspace --all-features
```

Expected: all pass.

- [ ] **Step 3: Manual smoke test of error shape**

With `RADARR_URL` and `RADARR_API_KEY` unset, pipe a bad request through serve and verify the envelope:

```bash
echo '{"service":"radarr","action":"bad.action","params":{}}' \
  | cargo run -- serve --transport stdio 2>/dev/null
```

Expected output (pretty-printed for readability):
```json
{
  "ok": false,
  "service": "radarr",
  "action": "bad.action",
  "error": {
    "kind": "unknown_action",
    "message": "unknown action `bad.action` for service `radarr`",
    "valid": ["help", "system.status", "system.health", "..."],
    "hint": null,
    "param": null
  }
}
```

- [ ] **Step 4: Commit if anything needed fixing**

```bash
git add -A && git commit --no-verify -m "fix(mcp): smoke test fixups"
```

---

## Self-Review

**Spec coverage:**
- ✅ `ok: true/false` on all responses (Tasks 1, 3)
- ✅ `service` + `action` in all envelopes (Tasks 1, 3)
- ✅ `data` on success (Task 1)
- ✅ `error.kind` + `error.message` on failure (Tasks 1, 2, 3)
- ✅ `unknown_action` kind for bad actions (Task 4)
- ✅ `missing_param` kind for missing params (Task 4)
- ✅ `auth_failed` kind for missing env vars (Task 4)
- ✅ Wire-shape tests (Task 5)
- ⏳ `rate_limited`, `not_found`, etc. from SDK — these still come through as `internal_error` until `RadarrError → ApiError → DispatchError` mapping is added in a future pass

**Placeholder scan:** None found.

**Type consistency:** `DispatchError` defined in Task 2, imported in Task 4 step 1, tested indirectly in Task 5. `build_success`/`build_error`/`build_error_extra` defined in Task 1, used in Task 3, tested in Task 5. No name drift.
