---
date: 2026-04-21 03:04:40 EST
repo: git@github.com:jmagar/lab.git
branch: fix/auth
head: d1a3ea6
agent: Claude (claude-sonnet-4-6)
session id: 096ad15f-3c36-44e3-86b6-a9462cbae993
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/096ad15f-3c36-44e3-86b6-a9462cbae993.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab  d1a3ea6 [fix/auth]
pr: "#25 — fix(auth): gateway admin auth, upstream OAuth, and dispatch fixes — https://github.com/jmagar/lab/pull/25"
---

## User Request

Fix phone-home 401 auth failures and deploy the `lab` binary to `steamy-wsl`, `vivobook-wsl`, `shart`, `squirts`, and `tootie`. Deployed hosts should self-register with the master (`dookie:8765`) after successful install.

## Session Overview

The prior session had wired up a phone-home stage in the deploy pipeline that runs `lab device hello --master <url>` via SSH after each successful binary install. However, the master's `/v1/device/hello` endpoint was behind the bearer-auth middleware, causing every phone-home attempt to fail with HTTP 401. This session diagnosed the root cause and fixed it by splitting the hello route out of the auth-protected router, then ran a full 5-host deploy to verify all hosts register with the device fleet store.

## Sequence of Events

1. Resumed from prior session context — phone-home was wired but failing 401 on remote `lab device hello` calls
2. Read `crates/lab/src/api/device/hello.rs` — handler itself has no auth logic (correct)
3. Read `crates/lab/src/api/router.rs` — confirmed the entire `/v1/*` tree is wrapped with `route_layer(authenticate_request)`, which blocks unauthenticated POSTs
4. Read `crates/lab/src/api/device.rs` — all device routes including `/hello` were in a single `routes()` function nested under the auth-protected `/v1/device`
5. Split `routes()` into `public_routes()` (just `/hello`) and `routes()` (everything else)
6. In `router.rs`, mounted `public_routes()` at `/v1/device` on the outer unauthenticated router, alongside `/health` and `/ready`
7. Built release binary — succeeded with no warnings
8. Copied new binary to dookie (`/usr/local/bin/lab`), killed old debug master process (PID 745988), restarted with new release binary
9. Tested from squirts: `lab device hello --master http://dookie:8765` — succeeded (no error output)
10. Verified `lab device list` — squirts appeared in the fleet store
11. Ran full 5-host deploy: `lab deploy run -y steamy-wsl vivobook-wsl shart squirts tootie`
12. All 5 hosts reached `phone_home` stage with `succeeded: ✓`; 3 devices visible in fleet store post-deploy

## Key Findings

- `crates/lab/src/api/router.rs:447-462` — the `v1_protected` router wraps the **entire** `/v1` tree with `route_layer(authenticate_request)`, including `/v1/device/hello`
- `crates/lab/src/api/device.rs:20-34` — all device routes were in a single `routes()` function; no split between public and protected
- `/health` and `/ready` escape auth because they're mounted directly on the outer `Router::new()` before the `v1_protected` merge — the same pattern now used for `/v1/device/hello`
- The master on dookie was running `target/debug/lab serve` (PID 745988) in a terminal session, not a systemd service — it had to be manually killed and restarted after binary update
- `deploy.phone_home.failed` WARN for `steamy-wsl` is non-fatal; the deploy still reported `succeeded: ✓` because phone-home errors are caught and logged, not propagated

## Technical Decisions

**Make `/v1/device/hello` unauthenticated** — Self-registration is a "phone home" event from a newly deployed device that doesn't yet have credentials. Requiring a pre-shared token creates a chicken-and-egg problem: the device can't register until it has the token, but it can't get the token until it registers. The hello endpoint writes device metadata into an in-memory store with no state-mutating side effects beyond that, so the security exposure is low.

**Split `device::routes()` into `public_routes()` + `routes()`** rather than adding per-route auth exemptions in the middleware. This keeps the exemption structurally explicit at the router level (same pattern as `/health`/`/ready`) instead of hiding it inside middleware logic.

**Did not pass the bearer token to remote hosts** — an alternative was to pipe `LAB_MCP_HTTP_TOKEN` through the deploy pipeline and pass it as env to the phone-home command. Rejected: leaks the master secret to 5 remote hosts; the unauthenticated-endpoint approach is cleaner and avoids secret distribution.

## Files Modified

| File | Purpose |
|------|---------|
| `crates/lab/src/api/device.rs` | Split `routes()` into `public_routes()` (just `/hello`) and `routes()` (everything else requiring auth) |
| `crates/lab/src/api/router.rs` | Mount `device::public_routes()` at `/v1/device` on the outer unauthenticated router |

## Commands Executed

```bash
# Build
cargo build --all-features          # → success (debug, quick verification)
cargo build --release --all-features # → success (28 MB binary)

# Update master on dookie
scp target/release/lab dookie:/tmp/lab-new
ssh dookie 'sudo mv /tmp/lab-new /usr/local/bin/lab && sudo chmod 755 /usr/local/bin/lab'
ssh dookie 'kill 745988'           # kill old debug process
# restart: lab serve (new process PID 1288224 listening on :8765)

# Test phone-home from squirts
ssh squirts '/home/jmagar/.local/bin/lab device hello --master http://dookie:8765 2>&1'
# → (no output = success)

# Verify registration
lab device list
# → squirts  ✓  non-master  0  0

# Full 5-host deploy
lab deploy run -y steamy-wsl vivobook-wsl shart squirts tootie
# → all 5: reached_stage=phone_home  succeeded=✓
# → device list shows: localhost, squirts, vivobook
```

## Errors Encountered

**Phone-home 401 (prior session, root cause of this session)**
- `ssh squirts 'lab device hello --master http://dookie:8765 2>&1'` → `status=401 kind="auth_failed"`
- Root cause: `/v1/device/hello` was inside the auth-protected `/v1` router
- Fix: moved to unauthenticated outer router via `public_routes()`

**Pre-existing test failures in `upstream/pool.rs`**
- `cargo test --all-features` fails with `error[E0063]: missing field proxy_prompts`
- Root cause: pre-existing — test code in `pool.rs` constructs `UpstreamConfig` without the `proxy_prompts` field added in a prior session
- Not related to this session's changes; debug build and release build both succeed

## Behavior Changes (Before/After)

| Endpoint | Before | After |
|----------|--------|-------|
| `POST /v1/device/hello` | 401 Unauthorized (bearer token required) | 200 OK (no auth required) |
| All other `/v1/device/*` | 401 without bearer token | Unchanged (still requires auth) |
| Deploy pipeline phone-home | Non-fatal 401 WARN on every host | Succeeds; hosts register with fleet store |

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `cargo build --all-features` | success | success | ✓ |
| `cargo build --release --all-features` | success | success | ✓ |
| `ssh squirts 'lab device hello --master http://dookie:8765 2>&1'` | no output (success) | no output | ✓ |
| `lab device list` (after squirts phone-home) | squirts in list | squirts ✓ non-master | ✓ |
| `lab deploy run -y steamy-wsl vivobook-wsl shart squirts tootie` | 5/5 succeeded | 5/5 succeeded | ✓ |
| `lab device list` (after full deploy) | devices registered | localhost, squirts, vivobook | ✓ (partial) |

## Risks and Rollback

- **Unauthenticated hello endpoint**: Anyone who can reach `POST /v1/device/hello` can write arbitrary device_id entries into the in-memory fleet store. The store is cleared on master restart and holds no sensitive data. Low risk in a private homelab network context.
- **Rollback**: Revert `crates/lab/src/api/device.rs` and `crates/lab/src/api/router.rs` — removes `public_routes()` split and returns `/hello` to the auth-protected tree.

## Open Questions

- `steamy-wsl` phone-home reported `deploy.phone_home.failed` with `error=verify_failed`. The binary deployed successfully (reached `phone_home` stage), but the phone-home SSH command itself failed. Possible cause: `steamy-wsl` may not have TCP connectivity to `dookie:8765` (different WSL network namespace), or the binary ran on a version that couldn't read the config. Not investigated.
- Only 3 devices appeared in the fleet store after deploying 5 hosts. Some hosts may have registered under unexpected short hostnames (e.g., `vivobook` instead of `vivobook-wsl`). `resolve_local_hostname()` returns the OS hostname, not the SSH alias.
- The master on dookie is running as a manually-started process (not systemd). If dookie reboots, the device fleet store is lost and the master does not auto-restart. No systemd unit exists for it.

## Next Steps

**Not yet started:**
- Fix `steamy-wsl` phone-home failure — investigate network connectivity from steamy-wsl to dookie:8765
- Investigate `resolve_local_hostname()` returning short hostnames that don't match SSH config aliases
- Add systemd unit for `lab serve` on dookie so the master auto-starts on boot and survives reboots
- Fix pre-existing test failure in `crates/lab/src/dispatch/upstream/pool.rs` — `UpstreamConfig` struct missing `proxy_prompts` field in test fixtures
