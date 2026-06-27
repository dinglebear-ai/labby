---
date: 2026-04-25 20:58:56 EST
repo: git@github.com:jmagar/lab.git
branch: bd-security/marketplace-p1-fixes
head: b1385289
agent: Claude (claude-sonnet-4-6)
session id: 8ceac97e-dec1-4351-b1f9-3b57834b06bf
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/8ceac97e-dec1-4351-b1f9-3b57834b06bf.jsonl
working directory: /home/jmagar/workspace/lab
pr: "29 — fix(marketplace): P1 security fixes — path traversal, symlink following, installPath validation — https://github.com/jmagar/lab/pull/29"
---

## User Request

Create scripts to test OAuth configuration (MCP endpoints, env/config, secured webapp endpoints, public OAuth endpoints). Then extend `lab doctor` with individual focused subcommands per check category, with richer output than the full audit.

## Session Overview

Explored the full HTTP auth middleware stack, clarified the OAuth + bearer token model, created `scripts/check-oauth.sh` (external black-box probe), ran it against `https://lab.example.com` (31/31 pass), updated `docs/OAUTH.md` and `docs/OPERATIONS.md`, then added `lab doctor auth`, `lab doctor system`, `lab doctor service <name>`, and `lab doctor services` subcommands with grouped section output and remediation hints. Fixed a pre-existing build break in `router.rs` and a HTML-dump UX bug in service findings.

## Sequence of Events

1. Read `router.rs`, `oauth.rs`, `serve.rs`, `.env.example` to map the full auth middleware and config surface.
2. Clarified the auth model: API and MCP both accept static bearer OR OAuth JWT; MCP rejects session cookies by design.
3. Created `scripts/check-oauth.sh` — 10-section curl-based external probe.
4. Ran script against `https://lab.example.com`: 30 pass / 1 fail / 1 warn.
5. Diagnosed the failure: `/auth/upstream/callback` probe sent forged `?state=csrf` which triggered real SQLite state lookup and returned `auth_failed` — not an auth gate failure. Fixed probe to send no params (expects 400/422).
6. Re-ran: 31/31 pass, 1 warn (MCP GET→400 expected).
7. Updated `docs/OAUTH.md` with "Verifying Auth Configuration" section; updated `docs/OPERATIONS.md` with `scripts/check-oauth.sh` entry.
8. Explored full doctor dispatch layer to understand patterns before coding.
9. Added `run_auth_checks()` to `dispatch/doctor/system.rs` — 10 checks covering mode, safety gate, bearer strength, public URL, Google credentials, auth store files, and Unix file permissions.
10. Wired `run_auth_checks()` into `stream_audit_full()` in `service.rs` (appears in full audit and `audit.full` MCP action).
11. Added `auth.check` `ActionSpec` to `catalog.rs` and both dispatch variants in `dispatch.rs`.
12. Exported `run_auth_checks` and `run_system_checks` from `dispatch/doctor.rs`.
13. Added `stream_service_probes()` to `service.rs` — service-only parallel probe without system/auth checks.
14. Rewrote `cli/doctor.rs` with `DoctorArgs` + `DoctorCheck` subcommand enum; per-subcommand rich output with section headers and grouped categories.
15. Changed `Command::Doctor` unit variant to `Command::Doctor(DoctorArgs)` in `cli.rs`; fixed and extended the two existing `Doctor` tests with 3 new parse tests.
16. Built and ran all 5 subcommands — discovered build break (`dev_mockup`/`dev_mockup_named` functions missing from router.rs after `dev_mockups.rs` was deleted upstream).
17. Fixed router.rs by removing the 4 dangling route registrations.
18. Discovered `lab doctor services` was dumping full Cloudflare 502 HTML into finding messages. Fixed `status_to_finding()` to truncate messages at 120 chars.
19. Fixed `lab doctor services` output: service name was not shown (all lines read `health: healthy (Xms)`). Added service name to the inline format.
20. All 5 commands verified working, 2292 tests passing.

## Key Findings

- `authenticate_request` (`router.rs:172`) tries three credential paths in order: static bearer → OAuth JWT → browser session cookie (v1 only). Both static bearer and OAuth are active simultaneously when both are configured.
- `/auth/upstream/callback` is mounted on the outer router outside the auth middleware, but the handler itself returns `kind:auth_failed` when the OAuth state token is not found in SQLite (`upstream_oauth.rs:447`) — indistinguishable from an auth gate rejection unless you probe without params.
- `run_system_checks()` (`system.rs:78`) and `run_auth_checks()` (`system.rs:194`) are disjoint — auth checks do not appear in `system.checks` action, only in `auth.check` and `audit.full`.
- `status_to_finding()` (`service.rs:388`) was putting the full HTTP response body (including multi-KB Cloudflare HTML error pages) into `Finding.message`. Truncated to 120 chars.
- `dev_mockups.rs` was deleted upstream but `router.rs` still registered `/dev` and `/dev/{name}` routes referencing the deleted `dev_mockup`/`dev_mockup_named` functions (`router.rs:695-698`), causing a build failure.
- Static bearer token grants hardcoded `lab:read + lab:admin` scopes unconditionally (`router.rs:195-199`); OAuth JWTs carry scopes from token claims.

## Technical Decisions

- **`run_auth_checks()` separate from `run_system_checks()`**: keeps check categories disjoint so `lab doctor system` and `lab doctor auth` each show only their domain. Both feed into `audit.full` via `stream_audit_full()`.
- **`stream_service_probes()` as new function**: `lab doctor services` needed service-only probes without system/auth noise. Extracted from `stream_audit_full()` rather than duplicating.
- **Shell script for external probe, CLI subcommand for internal pre-flight**: these are genuinely different — the script tests a live server from outside (CI, post-deploy); `lab doctor auth` checks config/files before starting. Not either/or.
- **Remediation hints in `Finding.message`**: avoids adding a `hint` field to the `Finding` type (would be a serialization break), and hints appear naturally in both human and JSON output.
- **120-char truncation for service messages**: prevents HTML error pages from flooding terminal output; still shows enough to identify the error type.
- **Removed dangling dev mockup routes rather than restoring the module**: `dev_mockups.rs` was intentionally deleted; the routes were a leftover from an incomplete refactor.

## Files Modified

| File | Change |
|------|--------|
| `scripts/check-oauth.sh` | Created — 10-section curl-based OAuth verification script |
| `docs/OAUTH.md` | Added "Verifying Auth Configuration" section with script usage and `lab doctor` guidance |
| `docs/OPERATIONS.md` | Added `scripts/check-oauth.sh` entry in "Repo-Level Helpers" |
| `crates/lab/src/dispatch/doctor/system.rs` | Added `run_auth_checks()` with 10 checks; added `#[cfg(unix)] file_perms_check()` helper |
| `crates/lab/src/dispatch/doctor/service.rs` | Added `stream_service_probes()`; wired `run_auth_checks()` into `stream_audit_full()`; truncated `status_to_finding()` messages at 120 chars |
| `crates/lab/src/dispatch/doctor/catalog.rs` | Added `auth.check` ActionSpec |
| `crates/lab/src/dispatch/doctor/dispatch.rs` | Added `auth.check` arm in both `dispatch()` and `dispatch_with_clients()` |
| `crates/lab/src/dispatch/doctor.rs` | Exported `run_auth_checks` and `run_system_checks` |
| `crates/lab/src/cli/doctor.rs` | Full rewrite: added `DoctorArgs`, `DoctorCheck` subcommand enum, per-subcommand handlers with section headers and grouped output |
| `crates/lab/src/cli.rs` | Changed `Doctor` unit variant to `Doctor(DoctorArgs)`; fixed 2 existing tests; added 3 new parse tests |
| `crates/lab/src/api/router.rs` | Removed 4 dangling `/dev` and `/dev/{name}` route registrations referencing deleted functions |

## Commands Executed

```bash
# External probe — first run
LAB_BASE_URL=https://lab.example.com bash scripts/check-oauth.sh
# → 30 pass / 1 fail / 1 warn

# External probe — after fixing callback check
LAB_BASE_URL=https://lab.example.com bash scripts/check-oauth.sh
# → 31 pass / 0 fail / 1 warn

# Full test suite
cargo test --workspace --all-features
# → 2292 passed, 3 ignored

# Live subcommand verification
./target/debug/lab doctor auth          # → 10 checks, all pass, grouped output
./target/debug/lab doctor system        # → 43 checks grouped by category
./target/debug/lab doctor service radarr # → healthy (2ms)
./target/debug/lab doctor service doesnotexist # → structured error, exit 1
./target/debug/lab doctor services      # → 20 services parallel, service names shown
./target/debug/lab doctor              # → full audit, 75 findings (unchanged behavior)
./target/debug/lab doctor auth --json  # → 10 findings JSON
```

## Errors Encountered

- **False-positive `/auth/upstream/callback` failure**: test sent forged `?state=csrf&code=authcode`; handler validates state token against SQLite and returns `auth_failed` when not found. Fixed: probe with no params to get 400/422 (missing required params = route is public and reachable).
- **Build break — `dev_mockup` not found**: `router.rs` still registered `/dev` and `/dev/{name}` routes after `dev_mockups.rs` was deleted. The 4 route registrations at `router.rs:695-698` were removed.
- **`unsafe` block in test**: `std::env::set_var` is `unsafe` in Rust 2024 edition; codebase has `deny(unsafe_code)`. Removed the env-mutation test; covered by the remaining structural tests.
- **`lab doctor services` showed no service names**: `print_finding_indented` stripped the check prefix but didn't include `f.service`. Fixed inline in `run_services()`.

## Behavior Changes (Before/After)

| Surface | Before | After |
|---------|--------|-------|
| `lab doctor` | Full audit only, no subcommands | Unchanged behavior when run without args |
| `lab doctor auth` | Did not exist | 10 auth checks grouped by category with remediation hints |
| `lab doctor system` | Did not exist | System checks only, grouped by category |
| `lab doctor service <name>` | Did not exist | Single service probe with section header |
| `lab doctor services` | Did not exist | All configured services in parallel with service names |
| `doctor({ "action": "auth.check" })` | Did not exist | Returns `DoctorReport` with auth findings via MCP/API |
| `doctor({ "action": "audit.full" })` | System + service probes | Now also includes auth checks |
| Service finding messages | Could contain full HTML error bodies | Truncated to 120 chars |
| `scripts/check-oauth.sh` | Did not exist | External black-box probe, exit 0/1 |

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `bash scripts/check-oauth.sh https://lab.example.com` | 31 pass, 0 fail | 31 pass, 0 fail, 1 warn | ✓ |
| `cargo test --workspace --all-features` | All pass | 2292 passed, 3 ignored | ✓ |
| `lab doctor auth` | 10 checks, grouped | 10 checks, 5 groups | ✓ |
| `lab doctor system` | System checks only | 43 findings grouped | ✓ |
| `lab doctor service radarr` | healthy (fast) | healthy (2ms) | ✓ |
| `lab doctor service doesnotexist` | structured error | `invalid_param`, exit 1 | ✓ |
| `lab doctor services` | service names + statuses | 20 services with names | ✓ |
| `lab doctor` (no args) | full audit unchanged | 75 findings streamed | ✓ |
| `lab doctor auth --json` | JSON DoctorReport | 10 findings JSON | ✓ |

## Decisions Not Taken

- **Mixing auth checks into `run_system_checks()`**: rejected because it conflates two distinct check categories, making `lab doctor system` noisier and redundant with `lab doctor auth`.
- **Adding `hint: Option<String>` field to `Finding`**: would require a serde change and break JSON consumers; hints embedded in `message` are sufficient.
- **`lab doctor check-oauth` as a new top-level command**: rejected in favor of extending the existing `doctor` subcommand hierarchy — `lab doctor auth` is more consistent with the established pattern.

## Next Steps

- **Not yet started**: add `just check-oauth` recipe as a one-liner wrapper around `scripts/check-oauth.sh` for post-deploy CI gates.
- **Not yet started**: scope-limited OAuth tokens — static bearer grants unconditional `lab:admin`; OAuth JWTs could be issued with narrower scopes for read-only clients.
- **Not yet started**: `lab doctor` full audit output interleaves tracing logs with findings when stderr is merged to stdout (e.g. `2>&1`). Not a bug in normal use (logs go to stderr, findings to stdout) but worth noting.
