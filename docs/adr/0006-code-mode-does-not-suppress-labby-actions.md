---
title: "ADR 0006: Code Mode Does Not Suppress Labby Atomic Actions"
created: "2026-09-24"
updated: "2026-09-24"
---

# ADR 0006: Code Mode Does Not Suppress Labby Atomic Actions

Date: 2026-09-24

Status: Accepted

## Context

ADR 0003 established Code Mode as Labby's bounded first-party execution plane and atomic MCP projection as the preferred representation for eligible first-party Labby operations.

PR #736 adds operation-specific atomic tools and exposes the same first-party capabilities to Code Mode through the `lab::*` namespace. Its transitional implementation still inherits the older `CodeModeVisibility::hides_raw_tools()` behavior: synthetic Code Mode visibility can suppress both generic routers and newly projected atomic Labby tools.

That is not the desired architecture. Code Mode is an orchestration surface, not a replacement catalog. A Labby action does not become less useful or less safe to expose directly merely because Code Mode can also invoke it.

## Decision

As of **2026-09-24**, enabling Code Mode **MUST NOT hide, suppress, or disable any otherwise-eligible Labby-owned atomic action solely because Code Mode is enabled**.

All eligible first-party `lab::*` actions remain available when Code Mode is enabled.

Code Mode is additive:

```text
direct atomic Labby tools
        +
Code Mode / lab::* orchestration
        =
the supported first-party surface
```

It is never a catalog-replacement rule:

```text
Code Mode enabled -> hide direct Labby actions
```

### Atomic availability is invariant across Code Mode visibility

For the same caller, route, project, and server configuration, the set of eligible Labby atomic actions must not shrink merely because the Code Mode synthetic surface is enabled.

An action may still be filtered by independent policy such as feature availability, route/project scope, service or action allowlists, caller authorization, product capability requirements, and action-specific approval or safety rules. Code Mode enablement itself is not such a policy gate.

### Code Mode is an additional invocation path

Direct atomic tools and Code Mode `lab::*` calls must derive from the same canonical action catalog, converge on the same dispatcher, and revalidate the same authorization, validation, destructive classification, approval/elicitation, error, and telemetry contracts at execution time.

Clients may use a direct atomic tool for one precise operation or Code Mode for discovery, orchestration, fan-out, reduction, and multi-step workflows. Labby must not choose between those surfaces by deleting one when the other is enabled.

### `codemode_read` remains read-only

This ADR does not weaken `codemode_read`. A read-only Code Mode execution may only discover/call actions allowed by its read-only safety contract. That execution-authority restriction must not cause the corresponding direct atomic tools to disappear from the normal MCP catalog for callers otherwise authorized to see them.

### Generic routers and upstream tools are separate policy concerns

Legacy service/action routers may have their own compatibility or deprecation policy. Upstream MCP tool projection may also have separate catalog-minimization policy. Neither policy may be reused implicitly to suppress first-party atomic Labby actions.

Any implementation flag such as `hide_raw_tools` must be narrowed or renamed so first-party atomic Labby actions are not treated as raw tools.

### Catalog size is not justification for suppression

Atomic tools provide operation-specific schemas, annotations, approvals, descriptions, observability, and a direct invocation path. Code Mode provides orchestration over those capabilities. The overlap is intentional.

Clients/operators that need a smaller catalog should use explicit route scopes, project scopes, allowlists, loadouts, or equivalent capability-selection mechanisms rather than Code Mode enablement as an implicit first-party filter.

## Required implementation behavior

1. `CodeModeVisibility::RootSynthetic` must not suppress eligible Labby atomic tools.
2. `CodeModeVisibility::InProcessPeer` must not suppress eligible Labby atomic tools merely because it exposes Code Mode.
3. Atomic publication must not pass through a legacy `hide_raw_tools` branch whose meaning is effectively `Code Mode is enabled`.
4. Direct atomic descriptors and Code Mode `lab::*` calls must derive from the same canonical action catalog.
5. Direct and Code Mode execution must converge on the same dispatcher and revalidate caller authority.
6. Enabling/disabling Code Mode may add/remove Code Mode's own synthetic tools but must not remove otherwise-eligible atomic Labby tools.
7. Route/project scoping, service/action allowlists, authorization, and destructive approval remain effective on both invocation paths.
8. `codemode_read` remains safety-filtered independently from top-level atomic publication.

## Verification requirements

Tests must prove that:

- enabling Code Mode removes no eligible atomic Labby descriptor;
- both `RootSynthetic` and `InProcessPeer` preserve atomic actions;
- Code Mode synthetic tools are additive to the first-party atomic catalog;
- direct atomic execution and Code Mode `lab::*` execution preserve action semantics and authorization;
- route/service/action allowlists filter both paths consistently;
- destructive approval cannot be bypassed by choosing Code Mode;
- `codemode_read` cannot execute mutating/destructive actions even while direct atomic tools remain published for appropriately authorized callers;
- descriptor parity tests prevent schema/annotation/output drift.

## Consequences

- Code Mode no longer acts as implicit first-party catalog compression.
- Atomic tools remain directly discoverable/callable when Code Mode is available.
- Code Mode remains useful without becoming mandatory indirection for simple operations.
- Tool catalogs may be larger; explicit scoping is the correct mechanism when reduction is needed.
- Existing logic that conflates raw/upstream tools with first-party atomic Labby actions must be separated.
- Future services inherit this rule automatically.

## Alternatives considered

### Hide atomic actions whenever Code Mode is enabled

Rejected. It turns Code Mode into a replacement surface and makes the product contract mode-dependent.

### Expose Labby actions only inside Code Mode

Rejected. It makes the orchestration runtime mandatory for operations already modeled as precise MCP tools.

### Let each service decide whether Code Mode replaces direct tools

Rejected. It creates inconsistent service-by-service catalog behavior.

### Keep `hide_raw_tools` and special-case services

Rejected. First-party atomic Labby actions are not raw tools; the category boundary must be fixed rather than accumulating exceptions.

## Relationship to ADR 0003

This ADR clarifies and amends ADR 0003. ADR 0003 remains authoritative for Code Mode as a bounded first-party execution plane. This ADR establishes that a preferred execution plane is not an exclusive tool surface. Where ADR 0003 can be read as permitting Code Mode to replace or hide eligible atomic actions, this ADR takes precedence.

## Authority and implementation status

This is an accepted architecture decision effective **2026-09-24**.

PR #736 is the active atomic projection implementation. Its current transitional use of `hide_raw_tools` for atomic publication does not satisfy this ADR and must be removed or narrowed before that behavior is considered complete.

## References

- [ADR 0003](./0003-code-mode-first-party-execution-plane.md)
- [GitHub issue #709](https://github.com/dinglebear-ai/labby/issues/709)
- [GitHub PR #736](https://github.com/dinglebear-ai/labby/pull/736)
- `crates/labby/src/mcp/catalog.rs`
- `crates/labby/src/mcp/handlers_tools.rs`
- `crates/labby/src/mcp/permanent_tools.rs`
- `crates/labby/src/mcp/peer_contract.rs`
- `crates/labby-gateway/src/gateway/code_mode/`
- `docs/dev/CODE_MODE.md`
- `docs/services/GATEWAY.md`
