---
title: "Unraid Core integration contract"
status: implemented-unbundled
version: 1
---

# Unraid Core integration contract, version 1

This contract defines Core's integration with an independently installed,
already-running Labby. It does not alter the existing standalone Labby Unraid
plugin, its `.plg` lifecycle, or Labby's independently deployable management
API. A future Core-bundled artifact is a separate packaging decision.

Unbundled protocol, provider, lifecycle, and test development may proceed in
isolated worktrees against an independently installed Labby. The licensing
gate below applies to packaging and release, not to that source-level work.

## Authority and process boundary

Labby remains the authority for MCP protocol handling, Streamable HTTP,
upstream connection lifecycle, Code Mode, sandboxing, upstream credentials,
and gateway-domain persistence. Core remains the authority for appliance
feature lifecycle, public endpoint policy, Core-session authorization,
delegated-actor issuance, native navigation, and Core audit projection.

The integration treats Labby as an independently owned process. Its trusted
host transport is a Unix-domain socket only; the process must not accept a
trusted-host assertion over TCP. UDS placement, peer identity, socket mode,
audience, expiry, key rotation, protocol version, and correlation headers are
all mandatory checks. A Core assertion never substitutes for Labby's own route,
capability, or destructive-action authorization.

The build profile is `--no-default-features --features integrated-gateway`.
That profile contains the gateway runtime, Code Mode, remote HTTP/OAuth
upstreams, the management API, and the gateway's required embedded web and
Skills surfaces. It excludes native filesystem and host administration,
systemd integration, API docs, and the standalone browser identity surface.
Runtime trusted-host mode additionally
requires `LABBY_INTEGRATED_TRUSTED_HOST=true`; startup rejects TCP, abstract
socket, missing peer-policy, or mixed bearer/OAuth identity configuration.
It also requires an absolute `LABBY_CORE_PROVIDER_SOCKET_PATH`; Core operations
are injected into Labby's existing Code Mode host under `unraid::*` and are
never registered as a second MCP upstream or implemented by a second sandbox.

Core's authenticated `GET`, `POST`, and `DELETE /mcp` ingress streams to
Labby's Unix socket. Labby calls Core in the opposite direction over the Core
provider socket. The public Core credential, Core browser cookies, Labby
management bearer, and upstream credentials never cross either private hop.
The delegated assertion is the only actor credential Labby receives.

Core supplies Labby's integrated environment as a non-secret profile: socket
paths/mode/ownership, accepted peer UID or GID, provider path, authority
generation, and current/previous public verifier keys. Core signing material
is never exported. A key-generation update requires an explicit Labby reload
or restart while the previous verifier key provides the bounded overlap.

## Protocol and capability rules

The integration has separate current (`2026-07-28`) and legacy
(`2025-11-25`) MCP compatibility profiles. Their initialize/session,
cancellation, and stream behavior are independently specified and tested.

Core capability discovery may inform Labby Code Mode only through a bounded,
actor-safe provider catalog and its search/describe/read execution contract.
Labby does not accept schema reflection as a public-tool definition, and Core
does not recreate Code Mode or expose generic GraphQL query/mutation tools.
Every provider operation requires explicit safety metadata, actor scope,
deadline, bounded result policy, and audit disposition. Core queries require an
explicit safe-query classification; plugin roots and sensitive roots are
hidden by default, and secret-shaped child fields are removed from generated
documents. Mutations are absent until explicitly classified.

The delegated assertion carries the parent MCP correlation ID. Each
`unraid::*` dispatch carries a distinct provider request ID so calls from one
Code Mode run can execute and cancel independently. When Labby's existing Code
Mode runtime drops an in-flight host call on cancellation or timeout, Labby
sends Core's matching `cancel` operation. Core distinguishes cancellation
before and after resolver dispatch and retains durable mutation-attempt state.

## Credentials and state

Public MCP credentials, Core browser sessions, delegated assertions, Labby
management credentials, Labby upstream OAuth credentials, provider
credentials, encryption keys, and artifact-signing keys are distinct trust
domains. No raw value crosses into another domain or into logs, metrics,
events, browser assigns, or diagnostic output.

Labby keeps its own configuration and credentials in a Labby-owned state path.
Core receives only versioned management and audit views. Core must never mount,
read, migrate, back up, or restore a Labby database by file access.

## Failure, release, and licensing gates

- A Labby failure rejects integrated gateway traffic without restarting Core.
  A Core UI/audit-projection failure leaves Labby's local management behavior
  independently recoverable.
- Any mandatory Core audit admission failure rejects destructive dispatch before
  Labby receives it. Read-only availability degradation is explicit and bounded.
- Future Core packaging needs pinned artifact digest/provenance, SBOM and notice
  checks, independent health/readiness, drained upgrade, migration/rollback
  proof, and clean-appliance qualification.
- **No Core-bundled artifact may be built, distributed, enabled, or advertised
  without a written `gateway-artifact-approval.json`.** The approval must
  identify an owner,
  date and expiry, Labby/Core version scope, AGPL/commercial terms, binary
  redistribution, embedded UI, source-offer obligations, notices, third-party
  assets, trademark, and support obligations. This does not gate attachment
  to an independently installed Labby.

Host-root compromise remains a residual threat; UDS hardening is necessary but
not sufficient. The paired Core document is maintained in Core as
`docs/architecture/mcp-gateway.md`.
