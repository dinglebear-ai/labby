# Code Mode Orchestration Master Plan

> **For agentic workers:** Implement this program through small, independently reviewable plans and PRs. Do not attempt the whole roadmap in one branch. Every phase below has an explicit gate and must preserve the current Code Mode execution contract unless the phase says otherwise.

**Goal:** Evolve Labby Code Mode from a secure one-shot composition engine into a durable, deterministic, policy-aware orchestration runtime while retaining its existing progressive discovery, generated TypeScript, sandbox isolation, route/tool scoping, snippets, artifacts, warm runner pool, and text-first MCP behavior.

**Architecture:** Keep **labby-codemode** host-neutral. Add effect semantics and replay decisions to its contracts, put durable storage and upstream identity in **labby-gateway**, and expose control operations through shared product dispatch in **labby**. Existing **codemode** calls remain immediate and backward-compatible by default. Durability and approval policies are explicit execution options, not a blanket destructive-call pause gate.

**Tech Stack:** Rust 2024, Tokio, serde, rusqlite WAL storage, sha2/HMAC, XChaCha20-Poly1305 using the existing workspace dependency, Javy/QuickJS, rmcp, Axum, clap, React/TypeScript inspector source under **apps/gateway-admin**.

## Status

- Planning date: 2026-07-31.
- Canonical plan location: **/home/jmagar/workspace/labby**.
- Baseline: local **main** at **eff39c79**, tracking **origin/main**.
- This is a program plan. Each numbered phase must receive a focused implementation plan before code begins.
- No implementation is included in this document.

## Relationship to older plans

This plan supersedes the product direction of these historical pause-first proposals:

- **2026-07-02-codemode-durable-pause-resume.md**
- **2026-07-02-codemode-pauses-sqlite-store.md**

Those documents remain useful research for replay cursors, monotonic sequence handling, HMAC integrity, owner checks, and restart recovery. They must not be implemented verbatim. Later Code Mode work intentionally removed the mandatory destructive-call pause/confirm contract. The governing compatibility rule is now:

> Current authorized calls continue to execute immediately. Approval is an optional policy result for explicitly matched calls, never an automatic second authorization gate applied to every destructive tool.

The existing notebook-as-log step journal is retained as the seed for durable execution storage, but its current read-only and redacted projection is not sufficient for exact replay.

## Current baseline that must be preserved

Labby already has the following mature Code Mode machinery:

- lexical and semantic search;
- focused, lazy describe-time type retrieval;
- generated TypeScript namespaces and raw callTool dispatch;
- route, upstream, tool, actor, and capability scope filtering;
- Javy/QuickJS with no ambient network, imports, Node APIs, or host filesystem;
- cleared runner environment, process-group or Job Object containment, non-dumpable Linux runners, per-execution filesystem jails, and contained artifact writes;
- warm runner processes with a fresh JavaScript runtime per execution;
- wall-clock, memory, stack, source, tool-call, snippet, step, artifact, result, and log budgets;
- snippets, nested snippets, snippet promotion, artifacts, local state/git providers, and hardened OpenAPI dispatch;
- redacted call traces, bounded in-memory history, an inspector MCP App, upstream MCP App capture, and scoped callbacks;
- codemode.step boundaries, step protocol messages, sequence numbers, and a SQLite-backed read-only journal projection.

The roadmap must extend these mechanisms instead of replacing them.

## Program decisions

1. **Replay before approvals.** Exact deterministic replay is foundational. Approval, cancellation, rollback, and fork operations depend on it.
2. **Effects are metadata, not authorization.** Effect classes inform scheduling, policy, retry, and UI. Existing scopes remain the authority for whether a caller may invoke a tool.
3. **Unknown is conservative.** Missing effect metadata is treated as an exclusive, non-retryable mutation for scheduling, but remains allowed under the default compatibility policy.
4. **No continuation snapshots.** Labby resumes by rerunning source from the beginning and replaying recorded values. It does not serialize a QuickJS heap or promise continuation.
5. **Exact private replay, redacted public projection.** Replay payloads require the exact result. History and UI consume a separately redacted representation.
6. **Policy snapshots are immutable per execution.** A resumed execution uses the policy and capability fingerprints captured when it began. Policy drift requires a fork or explicit administrator override.
7. **Approvals bind to one exact call.** An approval is scoped to execution id, sequence, tool id, canonical argument hash, actor, route, capability fingerprint, policy fingerprint, and upstream identity.
8. **Writes are deterministic barriers.** Read-only parallel work may fan out. Writes and unknown effects execute in stable sequence order.
9. **Rollback is honest.** Labby only offers compensation where an adapter or operator has declared a valid compensator. It never advertises universal transactional rollback for remote systems.
10. **Control plane is separate from code execution.** Execute remains codemode. Status, history, events, resume, approvals, cancellation, fork, rollback, doctor, and probe use shared dispatch control actions.
11. **Text-only remains the default MCP surface.** The inspector is opt-in and must not alter the ordinary Code Mode result envelope.
12. **Every new durable surface is bounded.** Rows, payload bytes, events, result sizes, retention, pagination, and concurrent resumes all receive explicit limits.

## Non-goals

- Replacing QuickJS with a different default runtime.
- Exposing host clients, credentials, environment variables, or raw filesystem access to sandbox JavaScript.
- Automatically retrying writes before exact replay and effect metadata are complete.
- Automatically pausing every tool marked destructive.
- Treating an MCP annotation as trusted authorization policy.
- Supporting arbitrary user-supplied rollback JavaScript in the gateway process.
- Building a distributed multi-node execution coordinator in the first release.
- Persisting model chain-of-thought or hidden client reasoning.
- Combining this program with unrelated gateway catalog, UI redesign, or deployment work.

## Target architecture

~~~text
MCP / CLI / HTTP
       |
       +-- codemode execute -----------------------------+
       |                                                  |
       +-- code_mode control actions                      |
                                                          v
                                               shared product dispatch
                                                          |
                                         +----------------+----------------+
                                         |                                 |
                               GatewayManager host                 inspector/event API
                                         |
             +---------------------------+---------------------------+
             |                           |                           |
      effect resolver             execution coordinator       catalog/identity snapshot
             |                           |
             |                  deterministic scheduler
             |                           |
             +------------------ decision engine --------------------+
                                         |
                            execute | replay | wait | deny | diverge
                                         |
                                  Javy/QuickJS runner
                                         |
                          upstream/local provider broker

Durable private store: executions, attempts, exact replay payloads, approvals
Durable public projection: redacted events, history summaries, notebook cells
~~~

## Program map

| Track | Outcome | Depends on |
|---|---|---|
| A. Effect contract | Stable effect, idempotency, retry, parallel, trust, and compensation metadata | none |
| B. Durable execution | Encrypted source and exact replay journal with attempts and lifecycle state | A contracts |
| C. Deterministic scheduler | Stable barriers and read-only concurrency | A, B sequence contract |
| D. Optional approvals | Exact-call pending actions, approve/reject, restart-safe resume | B, C |
| E. Events and history | Durable paginated history, event cursor, cancellation, fork | B |
| F. Compensation | Explicit reverse-order compensators and partial rollback reporting | A, B, C |
| G. Discovery expansion | Discoverable OpenAPI operations and adaptive exposure modes | A |
| H. Trust and operations | Upstream identity, status, doctor, probe, telemetry, evals | A through E |
| I. Deferred runtime expansion | Protocol-complete projections and optional higher-isolation runtimes | stable core |

# Contract 1: Tool effects

Add host-neutral types under **crates/labby-codemode/src/effect.rs** and re-export them from **lib.rs**.

~~~rust
pub enum ToolEffect {
    Read,
    IdempotentWrite,
    Write,
    Dangerous,
    Unknown,
}

pub enum RetryClass {
    Never,
    TransportOnly,
    Safe,
}

pub enum ApprovalMode {
    Allow,
    Deny,
    RequireApproval,
}

pub enum TrustClass {
    LocalBuiltin,
    LocalManaged,
    PrivateRemote,
    PublicRemote,
    Unverified,
}

pub struct ToolRuntimeSemantics {
    pub effect: ToolEffect,
    pub parallel_safe: bool,
    pub retry: RetryClass,
    pub approval: ApprovalMode,
    pub trust: TrustClass,
    pub compensation: Option<CompensationDescriptor>,
    pub provenance: SemanticsProvenance,
}
~~~

Resolution precedence:

1. exact operator override for upstream and tool;
2. built-in adapter declaration;
3. MCP tool annotations such as read-only, destructive, and idempotent hints;
4. current Labby destructive metadata;
5. conservative Unknown fallback.

MCP annotations are hints. They may reduce scheduling restrictions only when allowed by operator policy; they never grant caller authorization.

Extend these structures without removing existing fields during migration:

- **labby_gateway::upstream::types::UpstreamTool**;
- **labby_codemode::ToolDescriptor**;
- **CodeModeDiscoveryEntry**;
- generated TypeScript docs and describe output;
- inspector trace rows.

The first release records retry and compensation metadata but does not automatically retry or compensate.

# Contract 2: Execution lifecycle

Add host-neutral lifecycle types under **crates/labby-codemode/src/execution.rs**.

~~~rust
pub enum ExecutionStatus {
    Created,
    Running,
    AwaitingApproval,
    Completed,
    Failed,
    Rejected,
    Cancelled,
    Expired,
    RollbackRunning,
    RolledBack,
    RollbackPartial,
    RollbackFailed,
}

pub enum JournalDecision {
    Execute,
    Replay,
    AwaitApproval,
    Deny,
    Diverge,
}
~~~

Allowed transitions are explicit and tested. Terminal executions never return to Running; retrying or changing source creates a fork. Approval resumes the same execution with a new attempt number because its source and prior journal remain unchanged.

Each execution captures:

- immutable execution id and optional parent execution id;
- actor key, route scope, caller surface, and capability fingerprint;
- exact source, source SHA-256, and source format version;
- catalog fingerprint and upstream identity snapshot;
- policy fingerprint and effect-resolution version;
- creation, update, expiry, and terminal timestamps;
- current status, attempt number, replay cursor, and final error/result summary.

State transitions:

~~~text
Created -> Running
Running -> Completed | Failed | AwaitingApproval | Cancelled | Expired
AwaitingApproval -> Running | Rejected | Cancelled | Expired
Completed | Failed -> RollbackRunning
RollbackRunning -> RolledBack | RollbackPartial | RollbackFailed
any non-terminal -> Cancelled | Expired
~~~

Cancellation is cooperative at the host boundary and forceful at the runner boundary. A cancel request prevents new broker dispatches, terminates or evicts the runner, and records a terminal event. It cannot undo already completed remote writes.

# Contract 3: Replay decisions

Generalize the current step-only decision mechanism so every nondeterministic boundary participates in the same monotonic sequence spine:

- upstream tool calls;
- local provider calls;
- codemode.step boundaries;
- snippet resolution when the resolved source may change;
- optional future resource or prompt reads.

Add host-neutral decision inputs containing execution id, attempt, sequence, boundary kind, tool or step identity, canonical arguments hash, effect semantics, and captured identity/policy fingerprints.

The gateway decider returns:

- **Execute:** no matching prior journal entry exists and policy permits dispatch;
- **Replay:** the prior successful exact value matches the boundary identity and argument hash;
- **AwaitApproval:** policy requires a pending action for this exact boundary;
- **Deny:** policy explicitly refuses the call;
- **Diverge:** a prior sequence exists but identity, arguments, policy, capability, or upstream identity do not match.

Divergence is terminal for that attempt and must include a redacted diagnostic identifying which fingerprint differed. It must never silently re-execute the mismatched side effect.

When a decision is AwaitApproval, the parent does not send a normal tool error back into JavaScript. JavaScript could catch that error and continue. Instead, the parent records the pending action, terminates and evicts the runner, and returns a structured awaiting-approval execution result. Resumption reruns the source and replays earlier entries.

# Contract 4: Durable storage

Create **crates/labby-gateway/src/codemode_execution.rs** and a **codemode_execution/** directory. Move the existing step journal behind this module gradually; keep compatibility re-exports until all call sites migrate.

Recommended SQLite tables:

~~~sql
code_mode_executions
code_mode_attempts
code_mode_journal
code_mode_approvals
code_mode_events
code_mode_compensations
~~~

Core keys and invariants:

- execution ids are random, opaque, and never reused;
- attempts use a monotonically increasing integer within an execution;
- journal primary key is execution id plus sequence;
- events have a separate monotonic event sequence for cursor pagination;
- approvals are unique for execution id plus call sequence plus argument hash;
- every private row includes an integrity tag;
- every public projection is generated from already redacted values;
- all list operations are owner and route scoped before pagination;
- writes use transactions and prepared statements;
- WAL and synchronous NORMAL follow the existing journal pattern;
- database and key files are mode 0600 on Unix;
- retention pruning is bounded per sweep and never blocks execution dispatch.

## Private payload protection

Exact source and exact replay values are sensitive. Store them separately from redacted display data.

- Encrypt exact source, arguments when needed for replay, tool results, step values, and compensation captures with XChaCha20-Poly1305.
- Generate a stable 256-bit execution-store key atomically under the Labby home directory with restrictive permissions.
- Use execution id, sequence, payload kind, and schema version as authenticated associated data.
- Retain an HMAC or authenticated digest over canonical metadata so corruption and row swapping fail closed.
- Never return private payload columns from history, list, event, inspector, log, or tracing APIs.
- A missing key with an existing encrypted database marks durable execution unavailable. It must not create a new key and make old rows undecipherable.
- Key rotation is not required in the first slice, but the ciphertext envelope includes a key id and version so rotation can be added without schema replacement.

Reuse established crypto and atomic-file patterns from the repository instead of designing a new primitive.

## Proposed storage rows

**Execution row**

- identity and parent/fork information;
- encrypted source and source hash;
- owner, route, surface, and capability snapshot;
- catalog, policy, effect resolver, and upstream identity fingerprints;
- status, attempt, replay cursor, timestamps, expiry, and terminal summary.

**Attempt row**

- start and finish timestamps;
- runner/runtime version and pool disposition;
- resume source attempt and replay span;
- terminal kind, elapsed time, and aggregate counters.

**Journal row**

- sequence and boundary kind;
- tool id or step name;
- canonical arguments hash;
- effect and decision;
- encrypted exact outcome;
- redacted display outcome;
- success/error metadata, timing, and replay provenance.

**Approval row**

- exact call binding fields;
- redacted preview;
- requested, decided, and expiry timestamps;
- approver actor and decision reason;
- single-use consumed marker.

**Event row**

- event sequence, execution id, attempt, event kind, timestamp;
- compact redacted payload only;
- no private source, exact arguments, exact result, token, or credential material.

## Canonical hashing

Canonicalize JSON objects recursively by sorted keys while preserving array order and number/string distinctions. Hash a versioned envelope that includes boundary kind, tool id, canonical arguments, effect version, and local-provider operation version. The canonicalization version is stored per execution; changing it requires a new version, never reinterpretation of old rows.

# Contract 5: Deterministic scheduler

Add a host-neutral scheduling contract under **crates/labby-codemode/src/scheduler.rs** and implement it in the parent runner drive.

Rules:

1. JavaScript invocation order assigns sequence numbers.
2. Read plus parallel-safe calls may execute together only when no lower-sequence exclusive barrier is pending.
3. IdempotentWrite, Write, Dangerous, and Unknown are exclusive barriers by default.
4. Later reads do not pass an earlier write.
5. Completion may occur out of order for a read group, but journal commit and externally visible event order remain deterministic by sequence.
6. A replayed call consumes its original sequence without dispatching upstream.
7. AwaitApproval, Deny, Diverge, cancellation, or timeout closes the dispatch gate for all later sequence numbers.
8. codemode.batch remains a convenience API; the host scheduler decides actual concurrency.

Do not add automatic retry in this phase. A retry policy that appears in metadata is observability only until exact replay tests prove that duplicate dispatch cannot occur.

# Contract 6: Optional approval policy

Approval is a policy decision layered after existing authentication, route scope, capability scope, and schema validation. The default policy is Allow so existing authorized Code Mode behavior does not change.

Initial policy decisions:

- **Allow:** execute or replay normally;
- **Deny:** return a terminal policy error without upstream dispatch;
- **RequireApproval:** create one exact pending action and suspend the attempt.

Recommended configuration model:

~~~toml
[code_mode.approvals]
default = "allow"

[[code_mode.approvals.rules]]
upstream = "github"
tool = "merge_pull_request"
decision = "require_approval"

[[code_mode.approvals.rules]]
effects = ["dangerous"]
trust = ["public_remote", "unverified"]
decision = "require_approval"
~~~

Exact config field names must be finalized in the phase plan after inspecting the current config ownership types. The matching semantics are already decided:

- exact upstream/tool rules beat broader effect/trust rules;
- deny beats require-approval, which beats allow;
- rules can narrow by route and actor class but cannot broaden caller scopes;
- a policy may require approval for a read, or allow a dangerous call, because effect and authorization are separate concepts;
- policy compilation produces a stable fingerprint stored on the execution;
- invalid or ambiguous policy fails config validation rather than silently falling back.

Approval control actions:

- **approvals.list** with status, owner, route, and cursor filters;
- **approvals.get** for one redacted pending action;
- **approvals.approve** for the exact bound action;
- **approvals.reject** with an optional bounded reason.

Approve and reject require administrator authority initially. Later delegated approver roles may be added without changing the journal binding.

Approval consumption rules:

1. approving records a durable decision but performs no upstream call itself;
2. resume creates a new attempt and replays to the approved boundary;
3. the approved exact call executes once and marks the approval consumed in the same transaction that records dispatch intent;
4. a mismatch in tool, arguments, identity, route, actor, capability, or policy produces divergence and leaves the approval unused;
5. approval expiry produces an Expired or Rejected execution according to the configured retention policy;
6. rejecting is terminal for that execution; changing the decision requires a fork.

# Contract 7: Product surfaces

Keep the top-level **codemode** execution tool backward-compatible. Add only optional fields, for example:

- durable: false by default;
- execution_ttl_seconds with bounded defaults;
- policy_profile when profiles are introduced;
- client correlation metadata with strict byte and key caps.

A legacy request without these fields follows the existing one-shot path. It may still receive an execution id for trace correlation, but exact source and replay payload persistence is enabled only for durable execution.

Create shared control dispatch under **crates/labby/src/dispatch/code_mode/**. CLI, MCP, and HTTP adapters call the same dispatch functions.

Proposed actions:

- **executions.list**
- **executions.get**
- **executions.resume**
- **executions.cancel**
- **executions.fork**
- **executions.expire**
- **events.list**
- **history.list**
- **approvals.list/get/approve/reject**
- **rollback.preview**
- **rollback.execute**
- **status**
- **doctor**
- **probe**

Surface mapping:

- MCP: one Code Mode control service tool with action plus params, separate from execute;
- CLI: **labby code-mode** or the repository's final accepted naming, with matching subcommands;
- HTTP: versioned routes that call shared dispatch and support SSE for events;
- MCP App: private callbacks call the same dispatch with the current peer/route/actor context.

All control responses use cursor pagination and bounded redacted summaries. No control action returns encrypted private payloads or exact persisted source.

## Status response

Status should report:

- execution store configured/open/degraded;
- schema version and migration state;
- key id and key availability, never key material;
- journal/event row counts and oldest/newest timestamps;
- retention configuration and last prune result;
- runner pool size, busy, overflow, recycle, and wait metrics;
- semantic search availability and catalog fingerprint;
- policy fingerprint and rule count;
- pending approval count visible to the caller;
- durable execution feature flag state.

## Doctor response

Doctor is read-only except for temporary probe artifacts that are removed before return. Checks include:

- database directory and file permissions;
- encryption key permissions and decrypt/encrypt round trip using ephemeral data;
- schema integrity and foreign key checks;
- runner binary launch, fresh runtime, empty environment, and timeout enforcement;
- artifact jail traversal and symlink rejection probes;
- catalog search/describe coherence;
- policy compile and fingerprint stability;
- retention and quota configuration sanity;
- stale running executions that require recovery classification.

## Probe response

Probe defaults to internal checks only. An external upstream probe requires an explicit tool id whose resolved effect is Read, explicit params, and caller authorization. It executes once through the normal Code Mode broker and reports the tool id, effect resolution provenance, elapsed time, and redacted result shape.

# Contract 8: Events and durable history

Create an append-only event vocabulary independent of tracing logs:

- execution.created
- attempt.started
- catalog.snapshotted
- tool.queued
- tool.started
- tool.replayed
- tool.completed
- tool.failed
- step.recorded
- approval.requested
- approval.approved
- approval.rejected
- execution.awaiting_approval
- execution.cancel_requested
- execution.cancelled
- execution.completed
- execution.failed
- rollback.started/completed/partial/failed
- execution.expired

Event payloads are compact and redacted before persistence. Event rows use monotonic cursors and are returned in deterministic order.

Delivery mechanisms:

- MCP progress notifications when a client supplied a progress token and the negotiated peer supports them;
- HTTP SSE using Last-Event-ID or an explicit cursor;
- inspector polling or callback-based cursor fetch when the host cannot stream;
- ordinary execute response still contains the final bounded trace.

The existing in-memory CodeModeHistory remains as a hot cache and compatibility projection during migration. Durable history becomes the source of truth for list/get after its phase gate passes.

History APIs separate:

- execution summary rows;
- attempt summary rows;
- paginated redacted call/event details;
- notebook projection by step boundary.

Do not return all calls inline with every list response.

# Contract 9: Cancellation and fork

**Cancellation**

- sets cancel_requested transactionally;
- closes the scheduler dispatch gate;
- kills and evicts an active runner;
- records calls that completed before cancellation;
- marks the execution Cancelled;
- never claims completed remote writes were undone.

**Fork**

- creates a new execution id and parent link;
- copies or references the encrypted source through a new authenticated envelope;
- starts with a selected replay prefix from the parent;
- recomputes current policy, capability, catalog, and identity fingerprints unless an administrator explicitly requests the original snapshot;
- never mutates the parent journal;
- is the supported path for changing source, params embedded in source, policy profile, or capability set after a terminal result.

A basic fork starts from sequence zero with the same source. Prefix forks are introduced only after replay divergence tests cover partial ancestry.

# Contract 10: Compensation and rollback

Compensation is opt-in metadata plus host implementation. Add a host-neutral descriptor but keep argument construction outside sandbox JavaScript.

Preferred implementation model:

- built-in adapters implement a CompensationProvider trait;
- operator mappings use a restricted declarative mapper over captured forward args/result;
- arbitrary shell, JavaScript, templates with code execution, and credential interpolation are forbidden;
- compensation dispatch passes through normal authorization, route, identity, schema, effect, policy, tracing, and journaling paths.

Rollback workflow:

1. rollback.preview selects successful compensable write entries in reverse sequence order;
2. preview reports compensable, unsupported, already compensated, and blocked entries;
3. rollback.execute creates a dedicated rollback attempt;
4. each compensator is journaled before and after dispatch;
5. failure stops by default, with an explicit continue-on-error option for administrators;
6. terminal status distinguishes RolledBack, RollbackPartial, and RollbackFailed.

Rollback is never automatic on ordinary script failure in the first release. Explicit rollback avoids surprising secondary mutations while the feature matures.

# Contract 11: OpenAPI progressive discovery

The current OpenAPI provider is callable but its operations are not discoverable through the normal search/describe catalog. Preserve its hardened HTTP client, mandatory operation allowlist, server-side credential injection, and admin/unscoped gate while projecting allowed operations into discovery.

Represent each configured specification as a synthetic namespace:

- namespace: openapi plus a sanitized label, such as openapi_vendor;
- tool: sanitized operationId;
- id: openapi_vendor::get_user;
- helper: codemode.openapi_vendor.get_user(params).

Requirements:

- only explicitly allowed operations enter the catalog;
- label and operation collisions fail config validation;
- base URLs, credential values, and security scheme material remain host-only;
- input and output schemas are generated lazily using the existing describe path;
- large specs use a reduced search index and focused type retrieval;
- operation effect semantics come from explicit operator declarations first, then OpenAPI extension hints, then Unknown;
- route and local-provider gates are applied both during discovery and dispatch;
- hand-written raw ids cannot bypass the same visibility checks.

Add catalog fixtures for tiny, medium, and very large specifications and measure preamble size, describe latency, and search quality.

# Contract 12: Adaptive exposure modes

Support three operator-selectable catalog strategies after effect metadata and OpenAPI discovery are stable:

- **direct:** ordinary MCP tools are exposed directly to clients;
- **typed-code:** one Code Mode execution tool carries a generated namespace for a medium catalog;
- **search-execute:** a compact search/describe/execute surface fronts a very large catalog.

Automatic recommendation may consider tool count, generated type bytes, schema bytes, initialization latency, catalog churn, and trust class. Automatic mode switching is advisory in the first release. The configured mode remains authoritative so clients do not see unexpected tool-surface changes.

Record the selected exposure mode and catalog metrics in status and telemetry. Do not implement this track until OpenAPI operations share the standard discovery model.

# Contract 13: Upstream identity and trust

Separate mutable catalog fingerprints from endpoint identity.

Recommended identity snapshot:

- namespace;
- transport kind;
- normalized endpoint or command fingerprint without credentials;
- initialized server name and version;
- server-information hash;
- authentication subject class, not token or secret;
- trust class and trust provenance;
- optional operator pin.

Identity behavior:

- the same endpoint may change tool catalogs without becoming a different identity;
- an endpoint, command, server identity, or operator pin change blocks resume before any new upstream dispatch;
- administrators may fork under the new identity but do not mutate the original execution snapshot;
- public responses expose trust class and an opaque identity fingerprint, never raw private endpoints or auth subjects;
- local built-ins and managed local bridges receive explicit trust classes rather than relying on namespace naming.

Identity pinning begins as observability plus replay protection. It does not replace TLS, OAuth, route scope, or upstream authentication.

# Contract 14: Telemetry and evaluation

Add bounded metrics and tracing fields for:

- discovery index bytes, type bytes, preamble bytes, and build duration;
- lexical versus semantic search hits and semantic fallback rate;
- catalog cache hit/miss and fingerprint churn;
- runner checkout wait, overflow, recycle, crash, timeout, and eviction;
- scheduler queue depth, read-group width, barrier wait, and call completion skew;
- journal write latency, encrypted payload bytes, replay count, divergence count, and recovery count;
- approval wait duration, approval expiry, rejection, and resume attempts;
- event lag, SSE reconnect, history pagination, and retention pruning;
- compensation preview and rollback outcomes;
- result truncation, artifact fallback, and exact/private versus display payload sizes.

Never use tool arguments, exact results, source, approval reasons, or decrypted payloads as metric labels.

Create a first-class behavior harness with synthetic CodeModeHost implementations and optional live read-only fixtures. Scenarios must assert actual host dispatches rather than only final JavaScript output.

Required fixture groups:

- 10-tool, 250-tool, and 4,000-tool catalogs;
- lexical, semantic, ambiguous, stale, and scope-filtered discovery;
- sequential writes inside Promise.all;
- parallel read groups around write barriers;
- process crash after upstream success but before client response;
- service restart and exact replay without duplicate writes;
- source, argument, catalog, policy, capability, and identity divergence;
- JavaScript that catches ordinary tool errors after an approval boundary;
- cancellation during read fan-out and before a write barrier;
- approval consume races and duplicate approval requests;
- encrypted payload corruption, wrong key, and row swapping;
- retention while executions are active;
- partial compensation and retry-safe compensation;
- OpenAPI discovery at large catalog sizes;
- route-scoped callers attempting private history or approval access.

# Deferred contracts

These remain planned but must not delay the core durable runtime.

## Protocol-complete host projections

After tool replay is stable, evaluate host-mediated Code Mode APIs for MCP resources, prompts, roots, progress, and logging. Sampling and elicitation require independent loop, permission, and budget designs and are not automatically inherited from gateway protocol support.

## Higher-isolation execution class

Keep QuickJS as the default. Define an ExecutionClass abstraction only when a concrete workload needs a rootless container or MicroSandbox, such as untrusted extensions, controlled Python/data execution, or local providers needing explicit network/filesystem grants. The higher-isolation path must preserve the same decision, journal, event, and policy contracts.

## Static preflight

An optional parser-based preflight may reject disallowed syntax for stricter profiles and detect obvious obfuscation or pathological constructs. It is defense in depth and never replaces runtime isolation. It must not become a fragile allowlist for ordinary JavaScript.

## Credential references

Formalize host-only credential handles for connectors and OpenAPI operations. Code Mode receives opaque reference metadata only when needed for audit; it never receives secret values. This track should reuse the repository's existing authentication and secret-storage ownership rather than creating a second vault.

## Ecosystem bundle import

Map compatible external MCP server, skill, command, and prompt bundles into Labby's registry through an explicit preview/apply workflow. Imported Code Mode semantics default to Unknown and Unverified until an operator reviews them.

# Delivery sequence

## Phase 0: Freeze contracts and build fixtures

**Purpose:** Prevent the program from becoming a braid of incompatible partial implementations.

- [ ] Write an ADR for effects versus authorization and the default-Allow compatibility policy.
- [ ] Write an ADR for rerun-and-replay rather than VM continuation snapshots.
- [ ] Finalize status transitions, boundary kinds, canonical hashing version, event vocabulary, and error kinds.
- [ ] Define encrypted payload envelope and stable key lifecycle.
- [ ] Add synthetic catalog and host fixtures before production code.
- [ ] Capture current baseline behavior and performance for one-shot execution, batch reads, mixed calls, search, describe, and inspector traces.
- [ ] Mark the July pause-first plans historical/superseded without deleting them.

**Likely files:**

- docs/decisions or the repository's accepted ADR location;
- crates/labby-codemode/src/tests_*;
- crates/labby/tests/code_mode_runner.rs;
- docs/dev/CODE_MODE.md and docs/dev/ERRORS.md.

**Gate:** All new enums and error kinds are documented and fixture tests describe current behavior without changing runtime dispatch.

## Phase 1: Effect metadata and identity foundation

**Purpose:** Establish the shared language used by every later phase.

- [ ] Add ToolEffect, RetryClass, ApprovalMode, TrustClass, provenance, and compensation descriptor types.
- [ ] Extend UpstreamTool and Code Mode descriptors.
- [ ] Resolve MCP annotations and existing destructive metadata conservatively.
- [ ] Add operator override config with validation and stable fingerprinting.
- [ ] Compute upstream identity snapshots and expose only opaque fingerprints publicly.
- [ ] Include effect/trust in search, describe, trace, and inspector parsing.
- [ ] Add catalog cache fingerprint components so metadata changes invalidate the correct render.
- [ ] Add tests proving scope filtering still happens before metadata describe responses.

**Compatibility:** Default Unknown plus Allow preserves authorization behavior. Scheduling is not changed in this phase.

**Gate:** Every visible tool has deterministic effect and trust semantics with provenance, and current Code Mode integration tests remain green.

## Phase 2: Durable execution store and encrypted payloads

**Purpose:** Persist enough exact state to replay safely after restart without adding approval behavior yet.

- [ ] Introduce codemode_execution storage module and schema migrations.
- [ ] Add atomic key creation, encrypted envelopes, authenticated metadata, and restrictive permissions.
- [ ] Persist durable execution source and immutable snapshots only when durable is requested.
- [ ] Persist attempts, exact private boundary outcomes, and redacted display projections.
- [ ] Generalize the current step store into the new journal while preserving notebook projections.
- [ ] Add retention, quotas, startup recovery classification, and bounded pruning.
- [ ] Keep legacy executions on the in-memory path.
- [ ] Add store status and doctor checks, but no resume control action yet.

**Gate:** Restart tests decrypt and read durable rows, corruption fails closed, private payloads never appear in public projections, and ordinary one-shot calls produce byte-compatible results.

## Phase 3: Exact replay engine

**Purpose:** Make completed boundaries replayable with no duplicate upstream effects.

- [ ] Extend CodeModeHost with decision and record methods for all boundary kinds.
- [ ] Add Execute, Replay, and Diverge handling to runner_drive.
- [ ] Replay exact values before dispatch and record dispatch intent/outcome transactionally.
- [ ] Detect all fingerprint and argument divergences.
- [ ] Support resume after process restart for executions that failed after at least one committed boundary.
- [ ] Reuse the current source hash and capability ownership checks.
- [ ] Add explicit replay telemetry and attempt rows.
- [ ] Expose administrator-only resume experimentally through shared dispatch.

No approval or scheduler behavior changes yet; this phase is replay correctness only.

**Gate:** A test tool that increments an external counter is called exactly once across crash, restart, and resume, while JavaScript receives the recorded result on the resumed attempt.

## Phase 4: Deterministic scheduler

**Purpose:** Make mixed concurrency safe and repeatable before adding waiting approvals.

- [ ] Add per-execution sequence-aware scheduling state.
- [ ] Dispatch parallel-safe reads in groups.
- [ ] Treat write, dangerous, idempotent-write, and unknown calls as exclusive barriers initially.
- [ ] Preserve stable journal/event ordering independent of completion order.
- [ ] Close the gate on cancellation, timeout, divergence, or runner failure.
- [ ] Add scheduler metrics and saturation tests.
- [ ] Document codemode.batch as logical fan-out subject to host scheduling.

**Gate:** Repeated mixed-call scenarios produce the same dispatch order, the same replay journal, and no overlapping write barriers.

## Phase 5: Durable events, history, cancellation, and fork

**Purpose:** Turn replay internals into a usable execution lifecycle.

- [ ] Add durable redacted event rows and cursor pagination.
- [ ] Add execution/attempt/history list and get actions through shared dispatch.
- [ ] Add HTTP SSE and negotiated MCP progress delivery.
- [ ] Add cancellation state, scheduler gate closure, runner termination, and terminal events.
- [ ] Add basic same-source fork with a new execution id.
- [ ] Keep in-memory history as a hot cache and migrate inspector data sources gradually.
- [ ] Add retention behavior for source, exact journal payloads, public events, and summaries independently.
- [ ] Add owner, route, and capability tests for every control action.

**Gate:** A client can reconnect by cursor, observe a running execution, cancel it, inspect durable history after restart, and fork a terminal execution without accessing another actor or route's data.

## Phase 6: Optional approval policy

**Purpose:** Add human-in-the-loop waiting without restoring the retired blanket pause gate.

- [ ] Compile validated Allow/Deny/RequireApproval policies and stable fingerprints.
- [ ] Add AwaitApproval decision to the decider and non-catchable suspension to runner_drive.
- [ ] Persist exact pending-action bindings and redacted previews.
- [ ] Add list/get/approve/reject control actions.
- [ ] Resume an approved execution through exact replay and consume approval once.
- [ ] Add expiry, duplicate approval, approver race, policy drift, and identity drift tests.
- [ ] Extend inspector with pending action details and explicit approve/reject controls.
- [ ] Keep default policy Allow and durable mode opt-in through the entire phase.

**Gate:** A JavaScript program that catches tool errors cannot execute any call after an approval boundary, and the approved exact write dispatches once after restart-safe resume.

## Phase 7: Compensation and rollback

**Purpose:** Add explicit, honest recovery for declared compensable writes.

- [ ] Add compensation provider trait and restricted mapping model.
- [ ] Add metadata resolution and describe/inspector display.
- [ ] Add rollback preview with unsupported and blocked classifications.
- [ ] Add reverse-order rollback attempts and durable events.
- [ ] Add partial/failure state handling and operator reasons.
- [ ] Implement one built-in end-to-end compensator as the reference slice.
- [ ] Prove that ordinary script failure never triggers implicit rollback.

**Gate:** Preview and execution agree on selected entries, each compensator runs at most once, and partial rollback is reported without claiming transactional success.

## Phase 8: OpenAPI discovery and exposure strategy

**Purpose:** Apply progressive discovery to Labby's largest local catalog source.

- [ ] Compile allowed OpenAPI operations into synthetic Code Mode namespaces.
- [ ] Reuse lazy type retrieval and semantic search indexing.
- [ ] Resolve effect metadata for operations.
- [ ] Add collision, scope, credential-secrecy, and large-spec tests.
- [ ] Add catalog metrics and advisory direct/typed-code/search-execute recommendations.
- [ ] Keep exposure mode operator-controlled in the first release.

**Gate:** An allowed operation can be found, described, and called without the model knowing its label/operation id in advance, while denied operations and credentials remain invisible.

## Phase 9: Operational maturity

**Purpose:** Make the runtime diagnosable, measurable, and safe to enable broadly.

- [ ] Complete status, doctor, and probe on CLI, MCP, HTTP, and inspector surfaces.
- [ ] Add metrics listed in Contract 14 with bounded labels.
- [ ] Add the behavior/eval harness to CI with deterministic synthetic providers.
- [ ] Add soak tests for pool reuse, pruning, SSE reconnect, and repeated resume.
- [ ] Add backup/restore documentation for database plus key as one inseparable unit.
- [ ] Add incident runbooks for lost key, corrupt DB, stuck Running execution, approval backlog, and identity drift.
- [ ] Decide whether durable mode becomes the default only after production telemetry demonstrates acceptable overhead and storage behavior.

**Gate:** Doctor identifies intentionally broken fixtures, evals catch duplicate side effects and scope leaks, and operators can understand the state of the execution subsystem without reading raw logs or SQLite.

## Phase 10: Deferred expansion review

After the core program is stable, write separate decision records for:

- protocol-complete resource/prompt/root/progress/logging projections;
- sampling and elicitation loops;
- rootless container or MicroSandbox execution class;
- parser-based preflight profiles;
- credential-reference contracts;
- request coalescing or transport batching;
- ecosystem bundle import;
- prompt/cache fingerprint hints for clients.

No item in this phase is implicitly approved by this master plan.

# Expected file ownership

## labby-codemode

Host-neutral contracts and runner behavior:

- src/effect.rs
- src/execution.rs
- src/scheduler.rs
- src/policy.rs if policy matching can remain host-neutral
- src/host.rs
- src/types.rs
- src/protocol.rs
- src/runner_drive.rs and focused submodules
- src/preamble.rs and generated discovery types
- src/trace.rs
- src/config.rs

Keep storage, SQLite, upstream transport details, credentials, and operator config ownership out of this crate.

## labby-gateway

Gateway binding and durable infrastructure:

- src/codemode_execution.rs
- src/codemode_execution/store.rs
- src/codemode_execution/crypto.rs
- src/codemode_execution/decider.rs
- src/codemode_execution/events.rs
- src/codemode_execution/retention.rs
- src/codemode_execution/identity.rs
- src/codemode_execution/policy.rs
- src/gateway/code_mode/code_mode_host.rs
- src/gateway/code_mode/search.rs or current search ownership
- src/upstream/types.rs

Migrate current codemode_journal incrementally and retain temporary compatibility exports. Avoid a flag-day rename.

## labby product crate

Shared dispatch and adapters:

- src/dispatch/code_mode.rs plus focused modules;
- src/mcp/call_tool_codemode.rs for optional durable execute inputs;
- new MCP code-mode control service adapter;
- src/api/services/code_mode.rs;
- src/cli/code_mode.rs;
- config types and validation owned by the existing config module;
- result formatting and stable error-kind tests.

Business logic belongs in shared dispatch or gateway services, not copied among MCP, API, and CLI handlers.

## Inspector

Source of truth:

- apps/gateway-admin/components/code-mode-app/
- apps/gateway-admin/lib/code-mode-app/

Generated embedded assets are rebuilt through the repository's established workflow. Do not hand-maintain divergent inspector logic in the generated HTML.

# Error contract additions

Finalize names in Phase 0 and add them to the owning docs and all surface tests together. Candidate stable kinds:

- durable_execution_unavailable
- execution_not_found
- execution_not_resumable
- execution_expired
- execution_cancelled
- execution_awaiting_approval
- approval_not_found
- approval_expired
- approval_already_decided
- replay_diverged
- replay_payload_corrupt
- execution_identity_changed
- policy_snapshot_changed
- execution_store_key_unavailable
- rollback_not_available
- rollback_partial

Use existing kinds where their semantics already match. Do not create aliases that differ only by surface.

# Compatibility and migration

1. The existing codemode schema remains valid. New execution options are optional.
2. Non-durable execution retains current source retention, response, timeout, and authorization behavior.
3. Existing destructive permission checks remain in force. Approval policy does not substitute for them.
4. Default approval policy is Allow.
5. Existing ToolDescriptor fields remain during a deprecation window; new semantics are additive.
6. The current step_journal table is migrated or projected into the new model without destroying existing rows.
7. Existing notebook and history UI payloads remain parseable while versioned lifecycle fields are introduced.
8. Durable database migration is forward-only with startup backup or transactional migration safeguards.
9. The feature has independent kill switches for durable persistence, resume, approval, event streaming, and compensation.
10. Disabling a feature never deletes stored rows automatically.

# Security review checklist

- [ ] Exact source and replay values are encrypted and authenticated at rest.
- [ ] Database/key permissions are verified at creation and startup.
- [ ] Private payloads have no Serialize path into public response types.
- [ ] Redaction occurs before event/history persistence, not only on read.
- [ ] Actor, route, capability, policy, and identity checks happen before lookup results are returned.
- [ ] Approval controls are separate from execution and require administrator authority initially.
- [ ] Approval is bound to the exact canonical argument hash and consumed once.
- [ ] Resume fails closed on missing key, corrupt payload, scope drift, or identity drift.
- [ ] Cancellation closes dispatch before runner termination.
- [ ] Scheduler prevents later work from crossing a waiting/failed barrier.
- [ ] Local providers use the same replay and effect contracts where safe, with explicit exceptions documented.
- [ ] OpenAPI credentials, endpoints, and security schemes remain host-side.
- [ ] Metrics and logs contain no exact source, secrets, arguments, or results.
- [ ] Backup documentation treats key and database as an atomic security unit.
- [ ] Retention pruning cannot delete rows needed by active or awaiting-approval executions.

# Verification matrix

Every phase plan must select the applicable layers below.

## Unit

- effect resolution precedence and provenance;
- policy matching and fingerprint stability;
- canonical JSON and argument hashes;
- lifecycle transition legality;
- encrypted envelope round trips and tamper failure;
- scheduler barriers and deterministic grouping;
- redaction and public/private type separation;
- cursor pagination and retention selection;
- compensation selection order.

## Runner and host integration

- exact replay across fresh QuickJS runtimes;
- no duplicate upstream dispatch after crash;
- non-catchable approval suspension;
- cancellation while calls are pending;
- timeout/runner eviction with durable recovery state;
- local provider divergence;
- snippet and step boundaries sharing the sequence spine;
- artifact and UI capture behavior during replay.

## Surface

- MCP tool schema compatibility;
- CLI/MCP/HTTP parity through shared dispatch;
- route/actor/capability filtering;
- inspector old/new payload parsing;
- SSE reconnect and event cursor continuity;
- structuredContent and text result compatibility;
- error kind promotion.

## Adversarial

- swapped ciphertext rows;
- wrong or missing key;
- approval replay against changed args;
- concurrent approve/reject;
- policy and identity drift;
- JavaScript catch-and-continue attempts;
- Promise.all writes;
- source with inline secret-like strings;
- oversized source/result/event and quota exhaustion;
- symlink/path attacks against database/key/artifact locations;
- unauthorized cross-route history enumeration.

## Performance

Record before/after results for:

- trivial no-tool execution;
- one read call;
- 20 parallel reads;
- mixed 10-read/3-write program;
- 250 and 4,000 tool catalogs;
- durable encrypted journal overhead;
- replay of 10, 100, and 500 boundaries;
- event pagination and inspector rendering;
- startup recovery and bounded pruning.

# Rollout strategy

1. Land contracts and metadata with no dispatch behavior changes.
2. Ship durable storage behind an off-by-default feature/config gate.
3. Enable internal durable executions without resume for storage observation.
4. Enable replay for trusted-local administrators only.
5. Enable deterministic scheduler for durable runs, then measure before applying it to legacy runs.
6. Enable history/events and inspector lifecycle display.
7. Enable approval policies only for explicit rules.
8. Add one compensator and keep rollback administrator-only.
9. Expand OpenAPI discovery after catalog performance gates pass.
10. Consider broader defaults only after at least one release cycle of telemetry and incident-free use.

Each rollout step has a kill switch and rollback procedure. Database schema migrations are not rolled back by disabling runtime behavior.

# Suggested epic and issue decomposition

Create one program epic with these children rather than a single enormous issue:

1. Code Mode effect and trust contract.
2. Code Mode execution schema and encrypted payload store.
3. Code Mode exact boundary replay.
4. Code Mode deterministic effect scheduler.
5. Code Mode durable event/history control plane.
6. Code Mode cancellation and fork lifecycle.
7. Code Mode optional approval policy.
8. Code Mode compensation framework and reference adapter.
9. OpenAPI progressive discovery.
10. Adaptive exposure recommendations.
11. Code Mode upstream identity pinning.
12. Code Mode status/doctor/probe.
13. Code Mode telemetry and behavior eval harness.
14. Deferred runtime-expansion decision review.

Each child should produce its own implementation plan, tests, docs changes, migration notes, and explicit compatibility statement.

# Definition of done for the program

The program is complete when:

- an operator can opt into a durable execution;
- Labby can restart after a committed tool call and resume without dispatching it twice;
- mixed read/write concurrency follows deterministic effect-aware ordering;
- policy can require approval for one exact call without changing default legacy behavior;
- approval and rejection survive restart and remain actor/route/capability scoped;
- clients can list, inspect, stream, cancel, and fork executions through shared surfaces;
- supported compensations can be previewed and executed with honest partial outcomes;
- allowed OpenAPI operations participate in search, describe, and typed calls;
- upstream identity drift blocks unsafe resume;
- status, doctor, metrics, and evals expose runtime health and catch duplicate side effects;
- public history and inspector data contain no private replay payloads;
- the existing one-shot Code Mode contract and text-first MCP surface remain compatible.

# First implementation slice

The first coding plan should cover **Phase 0 plus Phase 1 only**. It must not create execution tables, approval actions, or scheduler behavior. Its deliverables are:

- final ADRs and stable contracts;
- effect/trust types and provenance;
- conservative metadata resolution;
- descriptor/search/describe/trace exposure;
- operator override validation and fingerprinting;
- synthetic fixtures and baseline measurements;
- full compatibility tests.

This slice gives every later phase a stable semantic bedrock without pulling the entire orchestration comet into one PR.
