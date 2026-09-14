# Q0 Product Qualification Inventory

Status: source inventory complete for Q0; Q1-Q6 remain qualification work.

This inventory maps the requested product qualification rows to evidence in
this checkout. It is a technical evidence record, not a task tracker. Beads
owns scheduling and completion state. The acceptance criteria remain in
[QUALIFICATION.md](QUALIFICATION.md).

## Evidence labels

| Label | Meaning |
| --- | --- |
| Real process | A compiled Labby or fixture executable crosses its public transport boundary with isolated state. |
| Real browser | Playwright drives Chromium against a real Labby process and built web assets. |
| Pinned provider | A real, version-pinned external-compatible service runs locally; this is not the public provider. |
| In-process | Production Rust components run in one test process without the complete product boundary. |
| Emulator | A socket, DOM, browser API, provider, or host test double exercises a protocol shape. |
| Contract only | Schemas, fixtures, or static checks are validated without executing the product journey. |
| Actual host | A dated journey crosses the real third-party provider or host boundary. |
| No evidence | No checked-in execution evidence exists for the named boundary. |

Only Real process, Real browser, Pinned provider, or separately recorded Actual
host evidence can satisfy the corresponding E2E boundary. Model/replay,
in-process, emulator, and contract-only results remain useful but receive no
product E2E credit. A required skipped test is a visible incomplete result, not
a pass.

## Existing harness and evidence contract

The canonical process harness is
[`crates/labby/tests/support/live_labby.rs`](../../../crates/labby/tests/support/live_labby.rs),
with authenticated journeys in
[`live_identity.rs`](../../../crates/labby/tests/support/live_identity.rs) and
the supervisor in
[`scripts/ci/labby-live-e2e.sh`](../../../scripts/ci/labby-live-e2e.sh).
The supervisor builds or accepts one exact binary, records its SHA-256 and
version, creates an isolated run root, runs bounded shards, retains hashed
evidence, and audits owned process groups, listeners, symlinks, file sizes, and
secret canaries. The orchestration self-tests are in
[`live_process_harness.rs`](../../../crates/labby/tests/live_process_harness.rs).

The PR tier runs contracts, HTTP/CLI/API, observability, IPv6, MCP parity, and
identity/protected-route/restart shards. Browser and fault-qualification shards
run only for nightly, manual, and release tiers. The release tier requires an
absolute packaged binary. The entry point is `just live-e2e <tier> <seed>`.

The real-browser supervisor is
[`live_browser_supervisor.rs`](../../../crates/labby/tests/live_browser_supervisor.rs).
It owns a real daemon, authenticated session, Chromium process, evidence files,
restart barrier, and cleanup while driving
[`live-backend.browser.test.ts`](../../../apps/gateway-admin/lib/browser/live-backend.browser.test.ts).
It qualifies Gateway Admin journeys; it does not install the browser extension
or emulate an OpenAI or Anthropic MCP Apps host.

Every new Q1-Q6 fixture must reuse these ownership rules and provide:

1. a ready acknowledgement only after its listener, catalog, and initial state
   are observable;
2. an effect counter or append-only effect ledger with a barrier before fault,
   cancellation, restart, or catalog mutation;
3. literal expected results independent of the production function under test;
4. source revision, binary digest/version, fixture name/version, seed, limits,
   transport, advertised protocol version, and non-secret auth subject;
5. request/correlation IDs, catalog generation and immutable artifact revision
   where applicable;
6. bounded stdout, stderr, traces, provider/browser versions, and redacted
   failure evidence;
7. separate build, ready, operation, idle, and teardown deadlines; and
8. proof that owned processes, sessions, sockets, listeners, temporary state,
   and retained credentials are gone.

Independent fixture workspaces must pin and audit their own lockfiles. A test
must not inherit a product-workspace audit result merely because it is launched
by the product harness.

## Q1: MCP protocol and native capabilities

| Requested row | Transport | Evidence lane | Current evidence and paths | Current decision | Remaining qualification gap | Owner |
| --- | --- | --- | --- | --- | --- | --- |
| MCP tools | stdio and Streamable HTTP MCP | Real process | Initialization, exact raw catalog, built-in help/calls, feasible action dispatch, structured redacted errors, project/read/admin denial, and cross-surface parity: [`live_mcp_actions.rs`](../../../crates/labby/tests/live_mcp_actions.rs), [`live_surface_parity.rs`](../../../crates/labby/tests/live_surface_parity.rs), fixture runner [`mcp_action_runner.rs`](../../../crates/labby/tests/support/mcp_action_runner.rs). | Supported. Existing core shard is genuine E2E. | Add explicit schema-invalid, unknown gateway/tool, destructive-policy, cancellation, and effect-counter cases rather than inferring them from catalog sweeps. | Q1, reusing `lab-jpo9u.11`. |
| Resources and prompts | stdio child through Streamable HTTP MCP proxy; direct production component calls | Real process for representative proxy journey; In-process for broader matrix | The proxy lists and reads a text resource and lists/gets a prompt in [`stdio_proxy_runtime.rs`](../../../crates/labby/tests/stdio_proxy_runtime.rs) with [`stdio_mcp_fixture.rs`](../../../crates/labby/tests/fixtures/stdio_mcp_fixture.rs). Pagination, exposure, ownership, stale-generation, notification, and timeout checks are under [`crates/labby-gateway/src/upstream/pool/`](../../../crates/labby-gateway/src/upstream/pool/). | Resources and prompts supported; current E2E is representative, not matrix-complete. | Real-process text/blob, templates, completion where advertised, invalid args/URI, collisions, bounded cursors, oversize results, and list-change behavior. Unsupported completion/template paths need literal method/capability decisions. | Q1. |
| Skills over MCP | stdio MCP, Streamable HTTP MCP, and Labby-to-Labby upstream federation | Real process | Real servers perform `skills/list`, `skills/get`, manifest-bound text/blob `resources/read`, unknown-skill rejection, OAuth-wrapped HTTP, federation, and trust rejection: [`skills_mcp_e2e.rs`](../../../crates/labby/tests/skills_mcp_e2e.rs), [`skills_oauth.rs`](../../../crates/labby/tests/support/skills_oauth.rs), [`standalone_stdio_skill_library.rs`](../../../crates/labby/tests/standalone_stdio_skill_library.rs). | Supported as native custom methods plus resources. This suite is not currently a live-E2E shard. | Add it to bounded qualification; cover disabled capability, subject isolation, namespace collision, pagination and a multi-hop proxy. Do not invent a different skills method. | Q1 and Q5. |
| Elicitation | MCP server-to-client request and relayed upstream MCP request inside production components | In-process | Ordering and relay behavior include client capability advertisement and destructive confirmation in [`elicitation.rs`](../../../crates/labby/src/mcp/elicitation.rs), [`call_tool.rs`](../../../crates/labby/src/mcp/call_tool.rs), [`call_tool_upstream.rs`](../../../crates/labby/src/mcp/call_tool_upstream.rs), and gateway relay tests under [`upstream/pool/`](../../../crates/labby-gateway/src/upstream/pool/). | Supported when the client advertises compatible elicitation; unsupported clients follow the server contract. No full E2E presently qualifies it. | Real client accepted, declined, cancelled, malformed, timeout, disconnect, correlation, destructive ordering, and no-effect-after-refusal cases. | Q1 and Q5. |
| Protocol lifecycle | stdio and Streamable HTTP MCP; direct production component calls for advanced lifecycle paths | Real process for initialize/discover/cancel; In-process for tasks, notifications, relay, roots, and sampling | Initialization/discovery variants and orderly cancellation occur in [`skills_mcp_e2e.rs`](../../../crates/labby/tests/skills_mcp_e2e.rs) and [`stdio_proxy_runtime.rs`](../../../crates/labby/tests/stdio_proxy_runtime.rs). Capability decisions are guarded in [`crates/labby/src/mcp/server.rs`](../../../crates/labby/src/mcp/server.rs) and pool modules. | Mixed support; capability detection is authoritative. `resources/subscribe` legacy RPC is unsupported and withheld. | One real-process capability/version matrix for cancellation, progress, tasks, list changes, roots and sampling, with explicit tested rejection for every unimplemented method. | Q1, reusing `lab-jpo9u.11`. |

## Q2: Authentication and OAuth

| Requested row | Transport | Evidence lane | Current evidence and paths | Current decision | Remaining qualification gap | Owner |
| --- | --- | --- | --- | --- | --- | --- |
| Bearer auth | HTTP API, Streamable HTTP MCP, browser WebSocket/API, and HTTP MCP proxy | Real process | Absent/malformed/foreign authority, revocation between discovery and execution, two issuers/projects, subject isolation, restart narrowing, denials, and proxy acceptance: [`live_protected_routes.rs`](../../../crates/labby/tests/live_protected_routes.rs), [`authority_qualification_matrix.rs`](../../../crates/labby/tests/authority_qualification_matrix.rs), [`auth_admin_api.rs`](../../../crates/labby/tests/auth_admin_api.rs), [`stdio_proxy_runtime.rs`](../../../crates/labby/tests/stdio_proxy_runtime.rs). | Supported. | Reconcile expired and revoked token cases across read/admin/destructive actions; retain upstream counters proving denied calls had no effect and scan every artifact for credentials. | Q2. |
| Google inbound OAuth | HTTPS authorization/callback, token and JWKS provider calls exercised through deterministic HTTP doubles; actual-provider HTTPS is not run | In-process; No evidence for Actual host | Provider/component tests cover login/callback, state, PKCE, token/JWKS validation, refresh/revocation and credential-broker behavior: [`crates/labby-auth/src/google.rs`](../../../crates/labby-auth/src/google.rs), [`docs/runtime/OAUTH.md`](../../runtime/OAUTH.md), [`docs/design/GOOGLE_CREDENTIAL_BROKER.md`](../../design/GOOGLE_CREDENTIAL_BROKER.md). | Supported inbound provider. Actual Google is credential-gated/manual. | Add dated actual-provider evidence with registered callback, account subject, provider metadata, refresh/revocation and cleanup; never retain credentials. | Q2 actual-provider lane. |
| Authelia inbound OAuth | Real HTTPS/OIDC to pinned local container plus Labby MCP callback/token, browser-session, and native callback/poll routes | Pinned provider; In-process for deterministic failure suite | `authelia/authelia:4.39.10` exercises discovery, login, PKCE, token, JWKS, callbacks and timing: [`tests/authelia/README.md`](../../../tests/authelia/README.md), [`tests/authelia/run.sh`](../../../tests/authelia/run.sh), [`authelia_acceptance.rs`](../../../crates/labby-auth/tests/authelia_acceptance.rs); run `just test-authelia`. | Supported. Pinned-provider evidence is separate from public deployment evidence. | Retain fixture image digest and bounded evidence in the qualification report; test provider restart/revocation paths not already observable through the pinned flow. | Q2. |
| GitHub OAuth, inbound | Configuration parsing/validation only; no GitHub network transport exists for inbound identity | Contract only | The provider is a closed `Google \| Authelia` enum in [`docs/design/INBOUND_IDENTITY_PROVIDER.md`](../../design/INBOUND_IDENTITY_PROVIDER.md) and [`crates/labby-auth/src/config.rs`](../../../crates/labby-auth/src/config.rs). | Unsupported by product contract. | Add a configuration rejection test and an explicit exclusion. Q2 must not add a provider. | Q2. |
| GitHub OAuth, upstream | Generic OAuth discovery/authorization/token flows through deterministic HTTP fixtures; live GitHub HTTPS not exercised | In-process; No evidence for Actual host | Generic authorization-code OAuth with PKCE covers discovery/issuer/registration/resource indicators, refresh hints and restart state in [`docs/services/UPSTREAM.md`](../../services/UPSTREAM.md) and [`upstream_oauth.rs`](../../../crates/labby/tests/upstream_oauth.rs). | Unknown until live provider metadata and registration are checked. Depot GitHub ingestion is not OAuth evidence. | Credential-gated/manual inspection of the exact GitHub MCP endpoint, metadata, client registration and scopes; record a dated supported or unsupported result without changing product scope. | Q2 actual-provider lane. |

## Q3: Code Mode

| Requested row | Transport | Evidence lane | Current evidence and paths | Current decision | Remaining qualification gap | Owner |
| --- | --- | --- | --- | --- | --- | --- |
| Discovery/describe/execute, fanout and replay | Code Mode runner subprocess JSON-line protocol; shell CLI smokes; direct gateway component calls | Real process for runner/smokes; In-process for gateway persistence and reconstruction | Minimal-host evaluation, search/describe/call routing, `Promise.all`, partial failures, binary values, snippets, runner isolation/recovery and step replay appear in [`code_mode_runner.rs`](../../../crates/labby/tests/code_mode_runner.rs), [`tests/smoke-code-mode.sh`](../../../tests/smoke-code-mode.sh), [`tests/smoke-code-mode-state-git-v2.sh`](../../../tests/smoke-code-mode-state-git-v2.sh), and [`crates/labby-gateway/src/gateway/code_mode/`](../../../crates/labby-gateway/src/gateway/code_mode/). | Code Mode is supported; runner tests alone are not a full Labby/upstream E2E. | Client to real Labby to fixture upstream; dependent call must consume the actual earlier result; add deterministic partial failure, bounded concurrency/stress baseline, timeout/cancel, queue/output/memory/fuel limits, and no duplicate or post-fence effect counters. | Q3, reusing `lab-ykxu5.7`. |
| Code Mode primitives | Direct Code Mode/gateway Rust component calls and MCP handler/resource calls; no complete client-to-daemon transport | In-process | Catalog/resource reconstruction, visibility, persistence and MCP Apps link capture exist under [`crates/labby-gateway/src/gateway/code_mode/`](../../../crates/labby-gateway/src/gateway/code_mode/), [`crates/labby-codemode/`](../../../crates/labby-codemode/), and MCP resource/handler tests. | Mixed support, incomplete E2E. | Real prompt/resource/template retrieval as inert data, capability revocation, stale generations, hidden entries, subject/scope and URI ownership isolation, and native UI metadata. | Existing `lab-ykxu5.7`; Q3 owns only uncovered qualification. |

Q3 must establish finite latency and resource baselines before setting regression
thresholds. The stress lane records seed, workload, concurrency, success/error
counts, latency distribution, retained output sizes, and cleanup; it does not
use an unbounded soak.

## Q4: Browser bridge and MCP Apps

| Requested row | Transport | Evidence lane | Current evidence and paths | Current decision | Remaining qualification gap | Owner |
| --- | --- | --- | --- | --- | --- | --- |
| WebMCP bridge | Real daemon WebSocket/HTTP driven by a Rust socket client; mocked Chrome extension APIs in Node | Emulator against Real process boundary; Emulator for extension tests | Origin/Host/auth denial, pairing, observation, invocation, revocation, disconnect and admission bounds are in [`live_browser_bridge.rs`](../../../crates/labby/tests/live_browser_bridge.rs); mocked extension behavior is in [`apps/browser-extension/test/`](../../../apps/browser-extension/test/). | Bridge product exists, but neither lane proves the required installed-extension browser boundary. | Playwright/Chromium page with the packaged installed extension through the real gateway: discovery, invocation/result, reconnect, origin/subject denial, cancellation, tab close, extension unload and cleanup. Record extension digest/version. | Q4. |
| MCP Apps | Direct Rust MCP tool/resource handlers and Node DOM/component rendering; no actual host transport | In-process for metadata; Emulator for UI host; No evidence for Actual host | Tools/resources attach MCP Apps and OpenAI-compatible metadata; handler and UI tests are in [`handlers_tools.rs`](../../../crates/labby/src/mcp/handlers_tools.rs), [`handlers_resources.rs`](../../../crates/labby/src/mcp/handlers_resources.rs), and [`apps/gateway-admin/components/code-mode-app/`](../../../apps/gateway-admin/components/code-mode-app/). | Supported metadata/surfaces, but current evidence is in-process or host emulator. | Deterministic host emulators for resource MIME, render, callbacks, results, reauth, CSP/origin, disconnect and teardown; separately version-pinned actual OpenAI and Anthropic host journeys with capability detection. Neither may claim the other's behavior. | Q4; actual-host lane requires account access. |

The existing real-browser Gateway Admin test is not WebMCP installed-extension
evidence and is not an MCP Apps host. The Rust bridge client is a socket
simulator. Node DOM and Chrome API tests are emulators. Reports must preserve
all three distinctions.

## Q5: Proxy parity and failures

| Requested row | Transport | Evidence lane | Current evidence and paths | Current decision | Remaining qualification gap | Owner |
| --- | --- | --- | --- | --- | --- | --- |
| Tools, prompts and resources | stdio fixture child proxied to Streamable HTTP MCP | Real process | Clients list/call tools, list/read resources, list/get prompts, exercise lifecycle versions, fixed ports, bearer/OAuth metadata, SIGINT and reaping in [`stdio_proxy_runtime.rs`](../../../crates/labby/tests/stdio_proxy_runtime.rs) with [`stdio_mcp_fixture.rs`](../../../crates/labby/tests/fixtures/stdio_mcp_fixture.rs); contract: [`docs/contracts/stdio-mcp-proxy.md`](../../contracts/stdio-mcp-proxy.md). | Supported and partially E2E-qualified. | Add resource templates, blobs, metadata/correlation assertions, ownership collisions, and one multi-hop case with exact per-hop effect ledger. | Q5. |
| Skills, elicitation and MCP Apps | Real stdio/HTTP Labby federation for skills; direct relay/handler calls for elicitation and apps | Real process for separate skills journey; In-process for elicitation/apps | Skills evidence is in [`skills_mcp_e2e.rs`](../../../crates/labby/tests/skills_mcp_e2e.rs); elicitation/app relay behavior is in gateway relay and MCP handler tests. | Supported pieces exist; unified proxy parity is unqualified. | Exercise each primitive client to Labby to fixture upstream, including a multi-hop case, preserving metadata, correlation and subject namespace. | Q5. |
| Proxy failures | stdio child through Streamable HTTP MCP plus direct pool/fault-sentinel calls | Real process for cleanup/auth; In-process for selected cancellation, timeout, catalog, and sentinel checks | Evidence is in [`stdio_proxy_runtime.rs`](../../../crates/labby/tests/stdio_proxy_runtime.rs), upstream pool tests, and [`e2e_fault_qualification.rs`](../../../crates/labby/tests/e2e_fault_qualification.rs). | Partial. Fault sentinels prove detectors, not every product failure journey. | Real upstream auth failure, disconnect/reconnect, slow/oversize/malformed reply, cancellation, invalidation, partial failure, secret redaction and no-double-execution counters. | Q5. |

The fixture effect ledger must identify hop, request and correlation IDs and
record `accepted`, `started`, `effect_committed`, `reply_sent`, and
`connection_closed`. Barriers fire after `started` or `effect_committed`, so a
test can distinguish cancellation from an ambiguous completed mutation.

## Q6: Send to Labby and Depot ingestion

| Requested row | Transport | Evidence lane | Current evidence and paths | Current decision | Remaining qualification gap | Owner |
| --- | --- | --- | --- | --- | --- | --- |
| Canonical Send to Labby | Static contract fixtures and in-process Labby control-plane client tests; no executed Depot Discovery-to-Labby journey | Contract only; In-process | The contract in [`docs/contracts/depot-control-plane.md`](../../contracts/depot-control-plane.md) resolves the selected provider to the same-ID acquisition connection, requests `/api/artifacts/exact`, verifies components, then commits with `artifacts.import`; [`depot_control_plane_contract.rs`](../../../crates/labby/tests/depot_control_plane_contract.rs) and [`docs/contracts/fixtures/depot-control-plane/`](../../contracts/fixtures/depot-control-plane/) validate Labby's client contract. | Canonical entry point resolved. Current source evidence is not a completed send E2E. | Correct Depot checkout must drive real Discovery selection to exact acquisition, Labby import, persisted/retrievable result, exact revision/provenance, authority, duplicate/retry, invalid payload, interruption and cleanup. Record both repository revisions. | Q6 Labby plus separately tracked Depot owner. |
| Proposed connected delivery | Static JSON fixtures parsed by Rust conformance tests; no backend delivery transport | Contract only | `dinglebear.depot-delivery/v1` is explicitly proposed and unimplemented in [`docs/contracts/phabby/depot-delivery-v1.md`](../../contracts/phabby/depot-delivery-v1.md); [`phabby_delivery_conformance.rs`](../../../crates/labby-runtime/tests/phabby_delivery_conformance.rs) validates its fixture shapes. | Not the canonical current Send to Labby implementation and not E2E evidence. | Keep as contract-only evidence until a separately authorized implementation exists. Do not substitute it for the Discovery acquisition/import journey. | Separate future Phabby/Depot work. |
| Depot ingestion | Static contract fixtures, in-process Labby client checks, and UI component tests; no real or deterministic source-to-Depot-to-Labby transport | Contract only; In-process; Emulator for UI components | Evidence is limited to [`depot_control_plane_contract.rs`](../../../crates/labby/tests/depot_control_plane_contract.rs), [`scripts/check-depot-control-plane-contract.py`](../../../scripts/check-depot-control-plane-contract.py), [`scripts/qualify-unified-depot.sh`](../../../scripts/qualify-unified-depot.sh), and Gateway Admin artifact/depot components. It does not execute GitHub, skills.sh, `marketplace.json`, or `plugin.json` ingestion. | Unknown from this repository alone. | In the correct Depot repository: deterministic local source servers plus credential-gated real sources; source resolution, nested paths/pagination, immutable revision/provenance, persisted catalog, Labby use, malformed/duplicate/unavailable/rate-limit/partial recovery, traversal/SSRF/trust rejection. | Q6 Depot owner; Labby consumes versioned public contract only. |

The proposed Phabby delivery protocol must not replace the current Discovery
contract in Q6. Cross-repository qualification requires explicit authority to
inspect the correct Depot checkout and separate tracked changes there. This
Labby task authorizes neither Depot edits nor production delivery.

## External gates and finite lanes

| Gate | Required evidence | Failure treatment |
| --- | --- | --- |
| Google actual provider | Registered client/callback, dated provider metadata, non-secret account subject, lifecycle trace and cleanup. | Missing credentials or unavailable account is visible incomplete actual-provider evidence; deterministic tests may still pass. |
| GitHub upstream OAuth | Exact endpoint, provider metadata, registration mode, scopes, dated outcome and cleanup. | Record supported or unsupported. Never infer support from Depot GitHub ingestion. |
| OpenAI and Anthropic hosts | Host name/version, detected capabilities, rendered resource/callback result, policy/reauth and teardown evidence for each host separately. | Missing account/host is visible incomplete; emulator success cannot waive it. |
| Installed WebMCP extension | Packaged extension digest/version, Chromium version, granted origin, tab/session identity and full teardown. | Socket and browser-API emulators remain separate passing evidence only. |
| Depot | Correct repository remote/revision, contract compatibility, source fixture versions, target and exact artifact revision. | Wrong or unavailable checkout blocks only Depot-owned rows; do not edit a guessed repository. |
| Platform | Exact OS/architecture and supported transport/features. | Record a tested exclusion for impossible platform behavior; do not require unsupported Unix primitives on Windows. |

Use the lane caps in [QUALIFICATION.md](QUALIFICATION.md): five minutes for
lifecycle conformance excluding build, fifteen minutes per deterministic
process/browser job, forty-five minutes for bounded stress, sixty minutes for
release qualification, and thirty minutes per actual-host/provider journey.
Timeout, missing credential, unavailable host, exclusion, and skipped status
must remain present in the final evidence report.

## Q0 decisions

- Reuse the live-process supervisor and its evidence/cleanup model.
- Count the Gateway Admin Chromium test as real-browser UI evidence only.
- Count `live_browser_bridge.rs` as a real-daemon socket simulator, not an
  installed-extension E2E.
- Count model/replay, DOM, Chrome API, provider and host doubles only in their
  named non-E2E lanes.
- GitHub inbound OAuth is unsupported; GitHub upstream OAuth is unknown pending
  live registration and metadata inspection.
- The canonical current Send to Labby path is Depot Discovery exact acquisition
  followed by `artifacts.import`.
- The proposed Phabby Depot-delivery protocol is contract-only and cannot
  satisfy the current Send to Labby row.
- Actual providers, actual MCP Apps hosts, installed-extension qualification,
  and Depot-owned behavior retain explicit authority and availability gates.
