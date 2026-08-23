---
title: "Phabby Shared Control Plane"
status: accepted-target
created: "2026-08-22"
---

# Phabby Shared Control Plane

Phabby is the accepted target for one open-source Phoenix/OTP human control
plane shared by personal Labby and hosted Depot. This is a migration target,
not current deployed behavior. Until a route passes the parity and rollback
gates in the migration ledger, Labby's in-tree `apps/gateway-admin` static
export and Depot's current LiveViews remain authoritative.

The repository is bootstrapped privately at `dinglebear-ai/phabby` while
licensing, source-history, trademark, contribution, generated-contract, and
third-party asset boundaries are recorded. The intended product remains open
source; private bootstrap does not allow proprietary Depot behavior in Phabby.

## Product boundary

Phabby owns web experience and control-plane orchestration. It does not become
a second implementation of Labby or Depot business rules.

```text
                     Phabby
              Phoenix + LiveView
                       |
          +------------+------------+
          |                         |
   Phabby.Backends.Labby     Phabby.Backends.Depot
          |                         |
       Labby API                 Depot API
```

Phabby contexts coordinate versioned backend contracts. Backends remain
authoritative for resource authorization, destructive classification, domain
validation, persistence, and operation semantics. Phabby has no direct access
to Labby or Depot databases and never silently falls back between targets.

The same route and component tree supports personal mode with one explicitly
selected Labby, hosted Depot mode, and connected mode with Depot plus an
explicitly authorized personal Labby. Capabilities are runtime data rather than
product-name branches. They control presentation only; each backend authorizes
every request.

## Runtime and failure boundary

A personal installation is one product containing two independently supervised
runtime releases:

```text
Labby installation
|-- labby-core       Rust
|   |-- MCP and Code Mode
|   |-- native and filesystem integration
|   |-- sandboxing and security-sensitive boundaries
|   `-- canonical Labby API and domain operations
`-- phabby           bundled BEAM release
    |-- Phoenix, LiveView, PubSub, and Presence
    |-- web sessions and UI orchestration
    |-- backend lifecycle and event projection
    `-- supervised control-plane work
```

Native packages and Incus images version the Rust binary and BEAM release
together. The release includes ERTS. Labby and Phabby expose independent
health, readiness, and logs. CLI, MCP, and API remain usable when Phabby is
unavailable, and a Phabby failure does not restart the gateway.

The initial local boundary favors authenticated loopback HTTP or a Unix socket
plus a versioned event stream. It defines compatibility ranges, bounded
requests, deadlines, cancellation, structured errors, correlation, redaction,
reconnect/backoff, and health. Phabby never imports Rust internals or opens
Labby's SQLite files.

## OTP ownership policy

Phabby is intentionally a broader OTP application whose first consumer is its
Phoenix web interface. This creates a gradual migration foothold without
committing Labby to a rewrite.

Responsibilities may move behind the versioned contract when OTP has a clear,
measured ownership advantage. Strong candidates include durable jobs and
schedules, upstream connection lifecycle and reconciliation, retry/recovery,
device and session presence, notifications and event routing, Artifact sync,
delivery coordination, queues, health, circuit breakers, and distributed
control-plane coordination.

Rust remains the default owner for MCP protocol and proxy machinery, Code Mode,
sandboxing, filesystem/native integration, parsers, packaging primitives, and
other proven security- or performance-sensitive components.

Migration uses a strangler seam:

1. Phabby calls the existing Rust implementation through the internal contract.
2. Cross-language fixtures and conformance tests lock behavior.
3. A responsibility moves only after ownership, persistence, failure,
   performance, compatibility, and rollback boundaries are explicit.
4. Consumers retain the same contract while implementation ownership changes.
5. The old implementation is removed only after production-equivalent proof.

Processes require a runtime reason. Stateless backend adapters and capability
evaluation remain plain modules and data. Supervised processes are reserved for
persistent state, concurrent work, or fault isolation. External polling is
centralized and broadcasts tenant-scoped events; it does not run per LiveView.

## Web and native clients

LiveView is the application architecture. JavaScript hooks or client islands
remain appropriate for editors, terminals, large trees, graph visualization,
streaming viewers, drag-and-drop authoring, and other browser-native widgets.

Responsive browser delivery is the first mobile surface. Tauri 2 may provide a
thin desktop/mobile shell around the same application. Remote content receives
no native capability by default; every IPC command and origin is explicitly
allowlisted and narrowly scoped.

## Hard gates

- No direct Labby or Depot database access from Phabby.
- No duplicated backend domain or authorization semantics.
- No product-name conditionals in shared components.
- No implicit backend fallback or hidden mutation queue.
- No unscoped PubSub topics for tenant or account data.
- No per-LiveView backend polling.
- No route cutover without functional, authorization, deep-link, failure,
  responsive, accessibility, telemetry, deployment, and rollback parity.
- No Rust-to-Elixir ownership move without a contract and rehearsed rollback.
- No implemented claim without runtime, fresh-client, and production-equivalent
  evidence.

See [the migration ledger](./phabby-migration-ledger.md) for staged cutover.
