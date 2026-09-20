---
title: "ADR 0003: Use Code Mode as Labby's Bounded First-Party Execution Plane"
created: "2026-09-20"
updated: "2026-09-20"
---

# ADR 0003: Use Code Mode as Labby's Bounded First-Party Execution Plane

Date: 2026-09-20

Status: Proposed

## Context

Labby already has a Javy/QuickJS Code Mode runtime with live discovery,
schema validation, route-scoped visibility, execution budgets, cancellation,
result shaping, step journals, snippets, and an in-process MCP peer for eligible
Labby services. It also has `state` and `git` providers deliberately confined to
Labby-owned workspaces.

Agents still need direct host tools for bootstrap, recovery, approval-sensitive
writes, and capabilities that Code Mode does not expose. Meanwhile, broadening
the existing Code Mode workspace into ambient host access would erase a useful
isolation boundary. Treating Synapse as a prerequisite for ordinary local work
would also make a single-host Labby installation depend on a distributed
execution system.

The architecture therefore needs a clear boundary for first-party execution:
what Labby owns locally, how an operator admits a repository, how native Labby
operations appear in Code Mode, and where direct host tools remain appropriate.

## Decision

Code Mode is Labby's first-party execution plane for bounded discovery, reads,
fan-out, reduction, orchestration, and explicitly admitted mutations. It is not
a mandate to remove every native host tool.

Labby owns the local execution path. Synapse remains an optional backend for
remote execution, multi-host scheduling, persistent workers, and distributed
coordination. Local workspace reads, edits, bounded commands, and MCP
orchestration must not require Synapse.

### Preserve sandbox state and admit host workspaces explicitly

The existing `state` and `git` providers remain rooted under
`$LABBY_HOME/code-mode-workspaces`. Host repositories enter Code Mode through a
separate, revocable workspace grant containing at least:

- a canonical root and stable workspace or lease identifier;
- read-only or read-write mode, allowed subpaths, and operation classes;
- symlink, mount, and path-containment policy;
- byte, result, process, time, and concurrency budgets;
- caller and route ownership, expiry, cancellation, and revocation;
- audit identity and artifact destination.

Every operation fails closed when the grant is absent, expired, revoked, or
incompatible with the requested path or mutation. There is no fallback from an
admitted workspace to arbitrary host paths.

### Reuse one execution engine and one dispatcher

Eligible first-party Labby operations are projected as precise atomic MCP tools
with operation-specific schemas, annotations, approval classes, and telemetry.
The existing generic service/action router remains as a compatibility surface.
Both forms call the same canonical dispatcher and revalidate the current
descriptor, arguments, caller authority, route policy, and approval state at
execution time.

The existing in-process MCP peer is the projection mechanism. Labby does not add
a second execution engine or a parallel authorization path for first-party
operations. Caller-bound, HTTP-context-only, administrative, or unsafe content
operations remain excluded until their required context can be represented and
enforced.

### Roll out reads before writes

Code Mode becomes the preferred surface in this order:

1. discovery, read-only calls, reduction, and bounded fan-out;
2. idempotent or explicitly approved workspace mutations with preconditions and
   structured receipts;
3. supervised processes and sessions after cancellation, output, environment,
   secret, and approval semantics are qualified.

Direct host tools remain valid for bootstrap, recovery, unavailable
capabilities, and approval-sensitive writes. Code Mode cannot convert an
instruction or a tool annotation into authority, bypass an approval, or retry an
ambiguous mutation blindly.

### Bound orchestration and process execution

`codemode.batch` and `codemode.map` use a scheduler capped by server policy and
the remaining execution budget. They stop admitting work after cancellation,
deadline exhaustion, or fail-fast failure; preserve input indexes; report
structured partial failures; and account for every admitted tool call.

A command provider uses argv execution as its canonical form. Shell
interpretation is a separate higher-risk mode. Working directory, environment,
stdin, duration, idle time, output bytes, process count, and concurrency are
bounded by the workspace grant and server policy. Cancellation terminates the
entire process tree, and detached children are disallowed by default.

## Consequences

- Code Mode becomes a compact, auditable route for multi-call agent work while
  retaining explicit escape and recovery paths.
- Labby must maintain exact parity between atomic projections and compatibility
  routers because two policy implementations would create authorization drift.
- Host repository access becomes an inspectable capability rather than ambient
  filesystem authority.
- Read-only qualification can ship before the higher-risk mutation and process
  surfaces.
- Host profiles may prefer Code Mode, but their claims must match the controls
  actually offered by each agent host.

## Alternatives considered

### Require Synapse for all Code Mode execution

Rejected for normal local work because it adds a distributed dependency where
Labby can enforce the boundary itself. Synapse remains available when the work
actually needs distributed execution.

### Broaden the existing state workspace to arbitrary host paths

Rejected because it would silently change the meaning and trust boundary of an
already-confined provider.

### Expose only generic Labby action routers

Retained for compatibility but insufficient as the primary projection because
it obscures operation-level schemas, safety metadata, approvals, and telemetry.

### Force all writes through Code Mode

Rejected because direct calls provide a clearer approval boundary for many
mutations and remain necessary for bootstrap and recovery.

## Authority and implementation status

This ADR records the proposed architecture from [GitHub issue
#709](https://github.com/dinglebear-ai/labby/issues/709). Acceptance of the ADR
does not admit a workspace, authorize a command, or prove that the staged
implementation and host profiles are complete.

## References

- [GitHub issue #709](https://github.com/dinglebear-ai/labby/issues/709)
- `crates/labby-codemode/`
- `crates/labby-gateway/`
- `docs/dev/CODE_MODE.md`
- `docs/services/GATEWAY.md`
