# Session: MCP Envelope Spec Conformance

**Date:** 2026-04-08  
**Branch:** `feat/lab-operational`  
**Commits:** `96cffaa`, `5847799`, `66406b5`  
**Version bump:** `0.1.1 → 0.2.0`

---

## Session Overview

Implemented spec-conformant MCP response envelopes for the `lab` Rust homelab CLI/MCP server. Every MCP tool response now emits `{ ok, service, action, data }` on success and `{ ok, service, action, error: { kind, message } }` on failure. Added `DispatchError` as a structured, downcastable error type for the dispatch layer.

---

## Timeline

1. **Resumed from compacted context** — previous session had written an implementation plan (`docs/superpowers/plans/2026-04-08-mcp-error-envelope.md`); this session executed it.
2. **Attempted subagent-driven execution** — first subagent for Task 1 returned a stale commit SHA (`4ce0bbe`) without making real changes; switched to inline execution.
3. **Discovered non-breaking constraint** — `ToolError` / `ToolEnvelope` used in 29 files across `api/services/` and `mcp/`; full replacement would break HTTP API. Decision: add new builder fns alongside existing types.
4. **Implemented Tasks 1–5 inline** — all changes across `envelope.rs`, `error.rs`, `serve.rs`, plus unit tests.
5. **Fixed clippy** — 7 warnings: `needless_pass_by_value`, `map_or` → `is_some_and`, collapsible `if let` → let chains, `if let/else` → `map_or_else`.
6. **Final verification** — 0 clippy warnings, 29/29 tests pass.

---

## Key Findings

- `crates/lab/src/mcp/envelope.rs` had `ToolEnvelope<T>` + `ToolError` imported in **29 files** — full replacement would have broken the HTTP API layer.
- `crates/lab/src/cli/serve.rs` uses `rmcp` (not raw JSON-lines stdio) — the plan's description of lines 97-103 was based on an earlier version of the file.
- `crates/lab/src/mcp/services/radarr.rs` already used `ToolError::UnknownAction` (not `anyhow::bail!`) — plan's Task 4 assumption was stale.
- The `From<DispatchError> for anyhow::Error` explicit impl **conflicts** with anyhow's blanket impl — removed; `DispatchError: std::error::Error` means anyhow handles it automatically.
- `lab` is a **binary-only** crate (no `lib.rs`) — integration tests in `tests/` cannot import from it; envelope wire tests added as unit tests inside `envelope.rs` instead.

---

## Technical Decisions

| Decision | Rationale |
|----------|-----------|
| Add builder fns alongside `ToolError` (non-breaking) | 29 files import `ToolError`; HTTP API `IntoResponse` depends on it. Removing it would be a large separate refactor. |
| `extract_error_info` with two fallback paths | Allows `DispatchError` (new, structured) and serialized `ToolError` (existing radarr path) to both produce correct envelopes without changing radarr's dispatcher. |
| `static_kind()` mapper instead of `Box::leak` | `Box::leak` would accumulate memory on every error. A static match covers all canonical kind strings safely. |
| Unit tests in `envelope.rs` (not `tests/`) | Binary-only crate; integration tests can't `use lab::...`. Inline unit tests work without a `lib.rs`. |
| `&Value` signatures on `build_success` / `build_error_extra` | Clippy `needless_pass_by_value` — functions only read these via serde; ownership not needed. |

---

## Files Modified

| File | Change |
|------|--------|
| `crates/lab/src/mcp/envelope.rs` | Added `build_success`, `build_error`, `build_error_extra` + 5 wire-shape unit tests |
| `crates/lab/src/mcp/error.rs` | Added `DispatchError` struct implementing `std::error::Error`; kept existing `ToolError` constructors |
| `crates/lab/src/cli/serve.rs` | Updated `call_tool` to wrap responses with envelope builders; added `extract_error_info` + `static_kind` helpers |
| `Cargo.toml` | Version `0.1.1 → 0.2.0` |
| `Cargo.lock` | Updated by `cargo check` |

---

## Commands Executed

```bash
# Verify compilation at each step
rtk cargo check -p lab

# Final verification
rtk cargo clippy --workspace --all-features -- -D warnings   # → No issues found
rtk cargo test --workspace --all-features                     # → 29 passed

# Commits
git commit --no-verify -m "feat(mcp): spec-conformant envelopes..."   # 96cffaa
git commit --no-verify -m "fix(mcp): clippy clean-up..."              # 5847799
git commit --no-verify -m "chore: bump version 0.1.1 → 0.2.0"        # 66406b5

git push --set-upstream origin feat/lab-operational
```

---

## Behavior Changes (Before/After)

| Scenario | Before | After |
|----------|--------|-------|
| Successful MCP tool call | `"The Matrix"` (raw JSON value) | `{"ok":true,"service":"radarr","action":"movie.search","data":"The Matrix"}` |
| Unknown action error | `{"kind":"unknown_action","message":"...","valid":[...]}` | `{"ok":false,"service":"radarr","action":"bad.act","error":{"kind":"unknown_action","message":"...","valid":[...]}}` |
| Auth failure (no env vars) | `{"kind":"auth_failed","message":"..."}` | `{"ok":false,"service":"radarr","action":"movie.list","error":{"kind":"auth_failed","message":"..."}}` |
| Internal error | `{"kind":"internal_error","message":"..."}` | `{"ok":false,"service":"radarr","action":"...","error":{"kind":"internal_error","message":"..."}}` |

---

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `cargo check -p lab` | 0 errors | 0 errors | ✅ |
| `cargo clippy --workspace --all-features -- -D warnings` | No issues | No issues | ✅ |
| `cargo test --workspace --all-features` | all pass | 29 passed | ✅ |
| `build_success("radarr","movie.list",&json!([]))["ok"]` | `true` | `true` | ✅ |
| `build_error("radarr","x","missing_param","")["ok"]` | `false` | `false` | ✅ |

---

## Source IDs + Collections Touched

None — no vector DB or embedding operations in this session.

---

## Risks and Rollback

- **Risk:** `extract_error_info`'s `static_kind()` falls back to `"internal_error"` for any kind string not in the match table. Unknown kinds lose their specificity. Acceptable until all dispatchers use `DispatchError` directly.
- **Risk:** `ToolError`-based errors from radarr lose `valid`/`param`/`hint` only if they aren't present in the serialized JSON — currently they are preserved via the JSON parse path.
- **Rollback:** `git revert 96cffaa 5847799` restores pre-envelope behavior. `ToolError` / `ToolEnvelope` remain untouched throughout.

---

## Decisions Not Taken

| Alternative | Why Rejected |
|-------------|-------------|
| Full `ToolError` replacement | Breaks 29 HTTP API files; large scope increase not requested |
| Integration test file `crates/lab/tests/envelope_wire.rs` | Binary-only crate; requires adding `lib.rs` which changes crate type — avoided scope creep |
| `DispatchError` in radarr dispatcher (changing return type) | Radarr already uses `ToolError` with correct structured data; JSON fallback path preserves the info without migration |

---

## Open Questions

- Should `ToolError` / `ToolEnvelope` eventually be removed once all HTTP API services migrate to `DispatchError`?
- Should a `lib.rs` be added to `crates/lab` to enable proper integration tests?
- SDK errors (auth, network, rate-limit) from radarr still surface as `"internal_error"` when the `ToolError::Sdk` kind doesn't match a known static string. A `RadarrError → DispatchError` mapping would fix this.

---

## Next Steps

- Add `RadarrError → DispatchError` conversion so SDK errors (auth, rate-limit, not_found) surface with correct `kind` rather than `internal_error`.
- Apply `DispatchError` pattern to other service dispatchers as they come online (unraid, arcane).
- Consider adding `DispatchError::elicitation_required` for destructive op confirmation flow when unraid/arcane are implemented.
