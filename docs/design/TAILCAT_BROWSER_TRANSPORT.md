---
title: "Tailcat Browser Transport"
status: proposed
created: "2026-10-01"
updated: "2026-10-02"
---

# Tailcat Browser Transport

Review draft for lab-et7en. This describes intended behavior, not shipped code.

## Outcome and scope

A signed-in operator opens a Phoenix page, explicitly pairs a local Labby,
and invokes authorized Microsandbox tools through Tailcat's browser WASM.
Native Labby remains the authority for caller identity, upstream tools and VM
lifecycle. No Labby Rust WASM is required.

The first implementation consists of a native `labby-tailcat` crate, a narrow
product transport adapter, and a portable JavaScript client hosted by Depot's
existing LiveView. The accepted Phabby control-plane design remains the target
for shared Phoenix UI ownership; this adapter must be movable without changing
Labby's operation contracts. Depot remains an OAuth resource server.

Hosted Google authentication, a public ChatGPT HTTPS gateway, general terminals,
preview proxying, artifact transfers, a new identity provider and hosted DERP
are separate follow-ups. First acceptance uses the existing Depot login and
an explicitly approved local pairing; it does not promise one-click Google setup.

## Evidence and existing owners

- A real browser spike exercised Tailcat Go WASM to native Tailcat and a
  synthetic HTTP MCP fixture, with request construction by a small Rust WASM.
  This proves a transport seam, not authenticated product or VM behavior.
- `labby-primitives` as a whole failed browser compilation at getrandom; its
  canonical JSON and digest modules compiled and passed five native tests.
  Production integration uses JavaScript plus Tailcat WASM.
- `labby-gateway` already owns process-group guards and Windows job handling.
  Reuse their process supervision ownership rather than copy the logic.
- `labby-auth` and product access/authority runtimes own authentication and grants.
  `labby-tailcat` must not mint tokens or implement an alternative access store.
- Depot's `OperatorLive` and `LiveSession` authorize existing browser sessions.
  A Depot session does not automatically authorize a local Labby.
- Existing `labby-browser` is the browser WebMCP extension bridge. This is a
  different direction of communication and must not reuse its identities or
  imply extension consent grants access to sandbox operations.

## Architecture

```text
Depot LiveView --- existing signed session ---> Depot contexts
       |
       +--- JS hook / Tailcat WASM === encrypted tunnel === native helper
                                                          |
                                               restricted Labby listener
                                                          |
                                              shared auth + dispatch
                                                          |
                                               Microsandbox MCP upstream
```

Depot supplies UI and rendezvous orchestration; sandbox payloads use the browser
tunnel. DERP carries encrypted traffic. A browser key or tunnel address grants
connectivity only; Labby still authenticates and authorizes each operation.

## Native crate and process seam

`crates/labby-tailcat` owns typed lifecycle configuration, state and supervision
for one helper per paired browser session. It depends on runtime vocabulary and
the shared native spawn/cleanup owner, not product CLI, Axum or Depot.

Use a pinned, minimal Go helper wrapping the upstream Tailcat library. This is
preferable to scraping human-readable CLI output. Build its native binary and
browser WASM from one locked source revision with checksums and retained license
notices. Keep upstream source in its own Go module; do not vendor its dependency
tree into Cargo. Never fetch or execute a latest binary at runtime.

The helper has a bounded NDJSON control protocol on private stdin/stdout:
`start`, `ready`, `stop`, `stopped`, `error`. Version is `1`; unknown versions,
fields required for authorization, extra ready events and malformed frames fail
closed. Frames are capped at 64 KiB; startup deadline is 30 seconds. Address
and key material are secret fields and never included in Debug or ordinary logs.

Start receives exactly one resolved loopback address, one allowed browser public
key and an approved DERP map. It cannot accept arbitrary destination mappings,
`all`, exit-node, exec, SSH or file service arguments. The helper proxies only
that listener. Config uses an absolute executable path and an explicit state
directory; no shell interpolation or ambient PATH resolution.

State transitions: stopped -> starting -> ready -> stopping -> stopped, with
failed reachable from any active state. Status includes version, generation and
typed failure but no private keys or connection address. Cancellation arms
cleanup immediately after spawn. Graceful shutdown has a two-second bound,
followed by process-tree termination and child reaping. Drop must kill remaining
owned processes. Automatic restart is disabled initially; explicit retry creates
a fresh generation and invalidates the old capability.

Initial support is macOS and Linux. Unsupported platforms return a typed error;
Windows must pass native job-object and browser tests before being advertised.

## Pairing and authorization

1. The browser generates a session-only key, never saved in localStorage.
2. A local operator starts pairing, specifying the expected HTTPS UI origin.
3. The browser and local CLI show a fingerprint of the same browser key and
   pairing nonce. The operator approves it locally within five minutes.
4. Labby binds the grant to that key, installation, principal, UI origin and
   session generation. Explicitly select the allowed Microsandbox upstream;
   only its allowed tool descriptors are visible and callable.
5. The authenticated rendezvous exchange delivers the connection capability to
   the approved browser, with no credentials in query strings or analytics.
6. Stop, local grant revocation, pairing expiration or browser teardown closes
   the session. Renewing requires an active grant and explicit owner policy;
   it cannot resurrect a revoked generation.

The initial grant lasts at most 15 minutes. It is checked against current native
authority on every invocation, not just at pairing. Existing auth/access owners
issue the credential and evaluate scopes and resource constraints; the transport
adapter additionally enforces the approved upstream/tool subset below all
surfaces. `lab:admin` is never implied by being a Depot operator.

The browser receives a context-encrypted transport envelope, rather than the
underlying project credential. `labby-auth` seals that credential with the
existing native encryption key and binds installation, principal, peer key,
HTTPS UI origin, resource, upstream, generation and expiry as authenticated
context. Only the dedicated listener opens this envelope. Its bearer format is
not accepted as a product credential by ordinary HTTP/MCP authentication.
Opening it does not authorize an operation: the listener still revalidates
current source and child authority and delegates to the existing protected MCP
route. Envelope issuance and the restricted listener are implemented. Complete
Microsandbox browser acceptance and final integration review remain pending.

Do not forward Depot's own bearer token to Labby. No browser-held native admin
credential. A copied connection address must be insufficient without the allowed
peer identity and valid local grant. Relaunching Tailcat must not widen access.

## Product listener and browser protocol

Bind a dedicated ephemeral loopback listener containing only the approved MCP
projection and health/readiness route. Require the native grant for MCP. Reuse
shared MCP handling, tool descriptors and dispatch; do not proxy the entire
operator HTTP API or implement sandbox commands in the Go helper.

The JavaScript adapter implements HTTP framing over Tailcat read/write, including
Content-Length, chunked responses and bounded SSE. It preserves MCP session and
protocol headers, request IDs, errors, deadlines and cancellation. It does not
use the spike's EOF-only response parser. Maximum JSON message is 1 MiB, eight
concurrent operations per session, with shared product response budgets still
authoritative. Streaming can be long-lived with a 30-second inactivity deadline;
explicit progress/heartbeat resets it. No automatic retry of uncertain mutations.

LiveView's hook owns module loading and its connection. A preserved DOM island
survives patches; hook destruction closes streams, pending calls and Tailcat.
Reconnect starts a fresh transport only while its grant is valid. UI distinguishes
paired, connecting, ready, expired, revoked, offline and failed. No fallback to a
different local installation. Normal Depot navigation remains usable if WASM
fails. Lazy-load the approximately 6.2 MiB compressed Tailcat asset.

Pinned self-hosted assets include the matching Go WASM runtime. CSP allows only
the selected DERP WebSocket hosts and required WASM execution; do not broadly
enable arbitrary scripts. The browser map is the same approved map used by the
native helper. Configuration of a custom map is administrator-only.

## DERP decision

Support an explicit custom DERP map; do not turn Labby into a DERP server.
Browser connections currently relay through DERP, so production hosting needs a
capacity and bandwidth policy. A future `derper` deployment can be separately
supervised with HTTPS, admission policy, observability and abuse limits. Its
failure must not restart Labby. Relays see connection metadata and encrypted
traffic, not decrypted sandbox payloads. A local relay alone cannot make an
otherwise unreachable workstation reachable; the relay needs reachable hosting.

## Verification and acceptance

- Unit/contract tests: config rejects non-loopback/arbitrary targets, unsafe
  executable selection, missing identity, unsupported versions and oversized
  frames. Errors and status never disclose address/keys/tokens.
- Native lifecycle tests: missing binary, checksum/version mismatch, early exit,
  startup timeout, cancellation, drop, graceful stop and descendant cleanup.
- Authorization tests: unauthenticated/revoked/expired/wrong-peer/wrong-machine
  requests denied; unapproved tools and generic gateway APIs unreachable.
- Browser integration: real WASM to native helper to disposable authenticated
  Labby, using npx Microsandbox MCP as its upstream. Create a network-disabled
  VM, run a marker command, verify output and destroy it in guaranteed cleanup.
- Real browser tests cover LiveView patches, navigation cleanup, reconnect,
  stream cancellation, session expiry and no replay of a mutation.
- A self-hosted disposable DERP fixture makes protocol tests deterministic;
  one bounded real-relay qualification verifies browser WebSocket behavior.
- Run focused Rust tests, Clippy, feature slices, generated-contract/docs gates,
  and Depot's focused auth/LiveView tests before its required repository gate.
  Source/build, mocked tests and live VM/browser qualification remain separate.

## Rollout and rollback

Feature disabled by default. Opt-in pairing starts only after authentication,
peer identity and tool-subset validation succeed. No background daemon install,
production restart or public endpoint created by this work. Initial integration
has no durable keys or access-store migration; stop revokes the transient grant.
Disabling the feature stops helpers and listeners without altering existing
Funnel, browser-extension, CLI or MCP routes. Persistent pairing is a later
contract with explicit storage and revocation tests.

## Source references

- [Labby architecture](../ARCH.md)
- [Phabby control plane](./phabby-control-plane.md)
- [Browser extension contract](../services/BROWSER.md)
- [Tailcat upstream](https://github.com/tailscale/tailcat)
- [Custom DERP guidance](https://tailscale.com/docs/reference/derp-servers/custom-derp-servers)

Written-design approval is the next gate. Then produce the implementation plan
and select execution before product code, as required by the brainstorming skill.

## Planned file-free pairing completion

The next pairing adapter uses the pinned Tailcat node-key implementation's
`SealTo`/`OpenFrom` methods. No second authentication service or private-key
export is introduced. This section describes work in progress.

Depot creates a five-minute opaque pairing ID and a separate random exchange
capability, retaining only the capability hash. The authenticated browser owns
request creation, one-time delivery retrieval and discard. The native CLI asks
for the exchange code through hidden input or stdin; codes never appear in URL
queries, shell arguments, access logs or ordinary output. The code permits
fetching public request metadata and depositing one ciphertext, and grants no
native authority. Both surfaces show the same public-request fingerprint.

After explicit local approval using the existing project authority, the running
native helper seals the complete delivery to its already approved browser peer.
The address contains a preshared key, so encrypting only the grant is insufficient.
Depot receives only a version, sender node public key and bounded ciphertext.
The browser's existing ephemeral identity opens it and checks the address's
server public key against the sender. The decrypted receipt binds the pairing
ID, origin, browser peer, upstream, generation, expiry and approved relay map.
The normal protected MCP handshake follows; a rendezvous code cannot replace it.

Deposit and retrieval are single-use and bounded by size, deadline, session and
global capacity. Native publication failure or an uncertain deposit retires the
session and its grant. Browser refresh, teardown, expiry and discard clear the
exchange and in-memory keys. Existing private-file pairing remains a fallback.
