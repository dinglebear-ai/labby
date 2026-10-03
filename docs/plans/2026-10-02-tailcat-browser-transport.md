# Tailcat Browser Transport Implementation Plan

**Goal:** Connect an authorized Depot browser to local Labby sandbox tools using
Tailcat WASM and prove a disposable Microsandbox VM round trip.

**Architecture:** A small Rust crate supervises a pinned Go helper; Labby's
existing auth and dispatch remain authoritative. A portable JavaScript transport
and Depot LiveView hook own browser connections. Transport keys and operation
grants are independent.

**Tech stack:** Rust/Tokio, Go/Tailcat, JavaScript/Go WASM, Phoenix LiveView.

**Spec:** [Approved design](../design/TAILCAT_BROWSER_TRANSPORT.md).

Use `superpowers:executing-plans` for native execution or
`superpowers:subagent-driven-development` if selected by the user. Beads
lab-et7en owns task status; this document is an engineering plan, not a tracker.
Stored here because repository instructions protect `docs/superpowers/`.

## Global constraints

- Feature disabled by default; macOS and Linux initially. No public listener,
  background installation, production restart, or deployment.
- One helper per browser session; exactly one resolved loopback target and one
  allowed peer key. No arbitrary port maps, host shell, SSH or file services.
- Private NDJSON protocol v1, 64 KiB frames, 30-second startup deadline,
  two-second graceful shutdown, then process-tree termination and reaping.
- Pairing expires after five minutes; grant lifetime at most 15 minutes.
  Bind machine, principal, browser key, UI origin, upstream and generation.
- Per-request native authority and tool subset enforcement. No Depot bearer
  forwarded to Labby and no browser-held admin credential.
- Browser JSON messages at most 1 MiB; eight concurrent operations; streaming
  inactivity deadline 30 seconds. Preserve stricter existing product budgets.
- No retries of uncertain mutations; no automatic helper restart initially.
- Matching pinned native/browser source and Go runtime; immutable checksums,
  licenses, self-hosted assets. No runtime download of latest.
- Custom approved DERP map supported; hosting DERP is separate.
- Preserve Phabby's shared control-plane target and Depot's resource-server role.

## Review focus

1. IPv4-mapped IPv6, DNS names and malformed addresses cannot widen the target.
2. Cancellation between spawn and ready kills descendants and releases ports.
3. A copied address or valid Depot session cannot bypass a local sandbox grant.
4. LiveView patch/navigation cannot retain an expired connection or duplicate calls.
5. Partial HTTP frames, chunked SSE and network failure never replay a mutation.

## File ownership

Labby changes:

- `crates/labby-tailcat/src/{lib,config,protocol,supervisor,error}.rs`: transport
  lifecycle only, plus tests under `crates/labby-tailcat/tests/`.
- `tools/tailcat-bridge/{go.mod,go.sum,main.go,protocol.go,server.go}`: minimal
  upstream library wrapper, tests and browser adapter build.
- `packages/labby-tailcat-browser/{transport.mjs,http.mjs,wasm.mjs}`: portable
  browser client, with Node protocol tests and real-browser acceptance.
- `crates/labby/src/dispatch/tailcat.rs` and `dispatch/tailcat/`: native pairing,
  grant orchestration and lifecycle; reuse current auth/access owners.
- `crates/labby/src/api/tailcat.rs`: restricted listener adapter.
- `crates/labby/src/cli/tailcat.rs`: explicit pair/status/stop adapters.
- Workspace/member/feature/module declarations, packaging scripts and generated
  catalogs updated with the tasks that require them.

Depot changes, in an isolated checkout preserving current work:

- `lib/depot/tailcat_rendezvous.ex`: short-lived session-bound rendezvous.
- `lib/depot_web/tailcat_live.ex`: authenticated connection UI.
- `priv/static/assets/tailcat_hook.js` and existing `app.js`: hook integration.
- Router, explicit runtime config, application supervision and CSP owners.
- Focused context, LiveView and browser tests. Read nearer guidance before editing.

No changes to `labby-browser` extension identities, product Rust WASM, or hosted
OAuth. New process supervision must consume the gateway's public process guard;
if extraction is needed, move that owner once with unchanged upstream behavior.

## Task 1: Locked helper protocol and build

**Produces:** Native helper and browser assets from the same source; typed
Start/Ready/Stop/Stopped/Error v1 fixtures consumed by Task 2.

1. Add failing Go tests in `tools/tailcat-bridge/protocol_test.go`: version 2,
   oversized frame, missing peer, non-loopback/mapped external IP, unknown target
   and a second start all reject without opening a listener.
2. Implement `DecodeStart(io.Reader) (Start, error)` and
   `Run(context.Context, io.Reader, io.Writer) error`. Resolve loopback numerically;
   never accept DNS names or CLI passthrough. Logs contain fixed error codes only.
3. Wrap upstream Server with an allowlist containing exactly the approved peer;
   expose one configured port. Clean up connections on stop/cancel.
4. Pin the successful spike revision `b4dc28e8aa8936f0a90a41ad8293a64e3d6b645f`
   and its required Go version through the module/build manifest. Build browser
   code from this source, including a key-generation export rather than
   importing the demo's localStorage persistence. Lock transitive dependencies.
5. Add `scripts/build-tailcat-bridge.sh` producing native helper, WASM, matching
   wasm_exec.js, checksums and notices in a task-owned output directory.
6. Verify `go test -race ./...` in the Go module, native build and browser build.
   Add a real helper test proving wrong-peer refusal with no backend request.
7. Review and commit only this task's files when publication scope permits.

## Task 2: Native Rust lifecycle crate

**Consumes:** Task 1 protocol and checksum manifest.
**Produces:** `BridgeConfig::validate() -> Result<ValidatedBridgeConfig, BridgeError>`;
`Bridge::start(ValidatedBridgeConfig) -> Result<Bridge, BridgeError>` (async);
`Bridge::status() -> BridgeStatus`; `Bridge::stop(&mut self) -> Result<(), BridgeError>`
(async). Connection capability is a separate redacted, explicitly exposed type.

1. Add a workspace member with no CLI/Axum dependencies. Add failing tests for
   relative executable paths, symlinks/changed binaries, checksum mismatch,
   malformed/duplicate ready, invalid target and secret redaction.
2. Implement validated immutable configuration and serde protocol types with
   unknown-field rejection. Verify the artifact before spawn; guard against
   replacement between verification and execution using the existing spawn owner.
3. Start a process group through the gateway supervisor guard, immediately arm
   cleanup and bound stdout framing. Drain stderr without retaining raw secrets.
4. Implement deadline/cancel/stop/drop behavior and typed failure transitions;
   transfer process ownership once. Reap children after force termination.
5. Add executable fixture tests for early exit, malformed frame, startup timeout,
   cancellation before/after spawn, child descendants, slow stop and repeated stop.
6. Verify `cargo test -p labby-tailcat` and
   `cargo clippy -p labby-tailcat --all-targets -- -D warnings`; native helper test
   must show listener closure after stop. Unsupported platforms fail explicitly.
7. Review the crate dependency graph and commit this isolated deliverable.

## Task 3: Portable browser transport

**Consumes:** Task 1 pinned browser exports.
**Produces:** `TailcatClient.connect(capability, {signal})`,
`request(method, params, {signal})`, `close()` and structured status events.
Capability contains connection address, in-memory peer identity, grant,
generation, resource, expiry and approved map; never serialized to URLs/storage.

1. Add Node tests for split headers, UTF-8 byte lengths, Content-Length, chunked
   responses, SSE split across chunks, oversized data, wrong request ID,
   session headers, deadlines and eight-operation admission limit.
2. Implement `http.mjs` as an incremental bounded parser, `wasm.mjs` as a
   single-flight lazy loader, and `transport.mjs` as the MCP session owner.
3. Tests must prove cancellation closes the stream, close rejects pending calls,
   malformed frames fail closed, and a failed mutation is never automatically
   resent. Empty or expired capabilities fail before connecting.
4. Run the package's Node test command and a real browser-to-helper fixture test.
   Verify matching artifact checksum and boot/runtime failures surface clearly.
5. Commit the reusable client without any product Rust WASM dependency.

## Task 4: Local grants and restricted product listener

**Consumes:** `Bridge`, `TailcatClient`, current native authentication/authority.
**Produces:** shared pair/start/status/stop operations; an ephemeral listener
exposing only the authorized MCP projection and health/readiness.

1. Read CLI, API, MCP, dispatch and auth guidance before editing. Add failing
   tests in `crates/labby/tests/tailcat_authority.rs` for wrong machine/key/origin,
   expired/revoked grants, wrong upstream and attempts to invoke admin tools.
2. Implement `PairingRequest`, `ApprovedPairing` and session generation in shared
   dispatch, using native identity and credential issuance. Local approval shows
   the key+nonce fingerprint and chosen Microsandbox upstream. Pairing consumes
   a nonce exactly once within five minutes.
3. Grant lifetime is capped at 15 minutes; explicit grant verification occurs
   below invocation and listing on every request. Apply exact upstream/tool
   subset to shared descriptors and dispatch, including retained results.
4. Mount restricted MCP routes on an ephemeral loopback listener. Require native
   credentials; do not mount the general operator API or browser OAuth session.
5. Add opt-in CLI adapters `labby tailcat pair`, `status`, `stop`. Never start a
   helper unless the pairing and restricted projection are validated.
6. Test auth-disabled configuration refusal, unknown routes, direct hidden-tool
   calls, expired active streams, stop/revocation closing helper and listener,
   fresh generation on retry and refusal to fall back to another installation.
7. Run focused integration/CLI contract tests, feature slices, generated docs
   freshness and Clippy. Commit runtime plus its thin adapters together.

## Task 5: Depot rendezvous and LiveView hook

**Consumes:** native local approval protocol and portable browser client.
**Produces:** opt-in `/ui/sandboxes` using existing authenticated LiveView.

1. Read Depot's current runtime-config, static assets, session and CSP owners.
   Register a separate Depot Bead linked to lab-et7en before code changes.
2. Add failing context tests: cross-user session, public-mode anonymous session,
   stale key/generation, nonce replay, capacity exhaustion and expiration reject.
3. Implement a bounded `Depot.TailcatRendezvous` supervised context. Pairing
   records expire after five minutes; connection records at the native grant's
   expiry, no later than 15 minutes. Validate local approval possession and
   browser key against the original authenticated session; never auto-approve
   from a Depot principal. Control messages are schema/size/rate bounded.
4. Implement LiveView rendering and hook; reauthorize rendezvous access through
   `LiveSession` on each event. Use authenticated CSRF-protected POST for native
   approval exchange. No tokens in HTML attributes, query strings or logs.
5. Preserve the hook DOM island across patches and clean up on destroyed/logout.
   Add explicit connecting/offline/expired/revoked/failed states and recovery.
6. Self-host pinned assets and minimally permit selected DERP WebSocket hosts and
   WASM in CSP. No broad unsafe script relaxation. Config disabled by default.
7. Run focused ExUnit and browser tests covering patch/navigation, unauthorized
   access, logout, failed boot, reconnect and non-duplication. Run Depot's required
   `mix check`; report unavailable external gates rather than skipping silently.
8. Commit only Depot changes after exact diff and secret review.

## Task 6: Real browser/VM acceptance and rollback

1. Add `tools/tailcat-bridge/integration/` with an owned disposable DERP fixture,
   isolated Labby home and explicit process cleanup; preserve existing services.
2. Start authenticated Labby with `npx -y microsandbox-mcp@0.7.6` as the approved
   upstream. Browser locally pairs, initializes MCP and lists only allowed tools.
3. Create a network-disabled, no-host-mount VM, run an identity+marker command,
   assert success/output, then destroy and verify the instance list is empty.
   A cleanup guard runs on every failure; record remaining resources if it fails.
4. Repeat wrong-peer, expired-grant and revoked-during-stream probes against the
   actual native endpoint. No VM execution occurs for denied requests.
5. Run one bounded real-relay browser test. Save redacted evidence, source/artifact
   hashes and native/browser versions; separate deterministic and live results.
6. Disable the feature; verify helper/listener stopped and ordinary Labby/Depot
   routes still work. Keep production deployment separate from qualification.
7. Update architecture/service/user docs and generated contracts to implemented
   truth. Final handoff names exact commits, checks, limitations and cleanup state.

## Execution review

Recommend native execution in this session: tasks share protocol, identity and
generation contracts, so keeping one implementer reduces interface drift. An
independent review can follow when explicitly selected. Plan review and execution
method selection precede production implementation under the writing-plans skill.
