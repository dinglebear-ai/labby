---
title: "Project Context and Session Binding Contract"
created: "2026-08-22"
updated: "2026-08-22"
status: "design"
---

# Project Context and Session Binding Contract

## Purpose

Project-scoped authorization is only safe if every operation knows which Project it belongs to. Labby MUST NOT maintain a process-global or principal-global mutable "active project" that can be changed by another tab, CLI invocation, MCP client, or agent session.

Project context is request-, route-, or session-scoped and becomes part of the authorization, cache, runtime-binding, and audit identity.

## Core invariant

For any operation whose policy or runtime authority depends on a Project, the effective context is:

- authenticated Principal;
- Organization;
- exact Project;
- policy epoch/version;
- relevant gateway catalog generation;
- transport/session correlation identity where applicable.

Changing the Project produces a new authorization context. It is never a cosmetic UI filter over one ambient gateway session.

## No ambient project

Labby MUST NOT:

- store one mutable active_project value for the whole process;
- store one mutable active_project value shared by every session for a Principal;
- infer the most privileged Project when Project is omitted;
- accept a Project ID from an untrusted request without checking Organization and membership;
- allow nested Code Mode/upstream calls to substitute a different Project; or
- select a Project runtime binding solely from an upstream/tool name.

A user may have multiple Project contexts open concurrently without interference.

## Context selection

A surface may remember a user's last selected Project as a UX preference, but that preference is not authorization. Before execution, the selected Project is resolved to a canonical Project ID and revalidated for the authenticated Principal.

If an operation requires Project context and none is supplied/bound, Labby fails with a safe project-context-required error. It does not guess.

Operations that are genuinely Organization- or Personal-scoped may execute without a Project according to their own permission contract.

## HTTP/API

Project-sensitive HTTP/API operations SHALL bind Project context explicitly in the authenticated request contract, for example through a project-scoped route or a typed request field defined by the canonical API schema.

The server validates the Project against the authenticated Principal and Organization. A client-supplied identifier is only a selector, never proof of access.

If a browser UI remembers the selected Project in server-side/session state, state-changing requests still use the normal CSRF protections and the server resolves the bound Project for that request. Another browser session changing its selection must not mutate this session's context.

The exact public route/field shape should be frozen with the API implementation, not guessed in this design packet.

## CLI

The CLI resolves Project context per command.

A future UX may support:

- an explicit --project selector;
- a named local profile/context; or
- an interactive selection.

Regardless of UX, the resulting Project ID is sent/bound explicitly for the operation. A local CLI default does not grant access and cannot override server-side membership/policy.

Concurrent CLI processes are independent.

## Web UI

The UI exposes a visible Project/workspace selector when the user has multiple eligible contexts.

Changing Project:

1. resolves/revalidates the Project;
2. invalidates the old caller-specific workspace/catalog view in that UI session;
3. fetches/resolves the new EffectiveWorkspace;
4. updates route/context state; and
5. never mutates another browser tab/session's authorization context implicitly.

Where practical, URL/navigation state should identify the selected Project so links are reproducible and do not depend on hidden global state. Authorization still happens server-side.

## MCP

MCP Project context is stored in a server-created `BoundAccessContext`, not inferred from global state or accepted from tool arguments. It binds Principal and authenticator/credential, route, Organization, Project, access revision, expiry, and a safe fingerprint. Stateful transports create it at session establishment and validate it on every request/resume; stateless HTTP creates it per request. Stdio, Unix, test, and in-process transports require an explicit local/service Principal after enforcement.

MCP catalogs and sessions are stateful enough that Project context should be bound no later than session establishment and remain immutable for that MCP session.

Preferred v1 shape: expose a Project-bound protected/virtual gateway route whose resolved target includes the Project ID and its assigned Loadout/workspace policy. The exact route naming is an implementation/API decision, but the route binding itself is authoritative after authentication.

Alternative future MCP initialization metadata MAY carry Project selection only if it is authenticated, standardized/explicitly versioned, and bound immutably to the resulting session. Arbitrary per-tool-call metadata MUST NOT be allowed to switch Project under an already established Project-bound session.

Switching Project in an MCP client therefore requires a newly bound session/connection or another explicit protocol-level reinitialization that produces a new session identity and workspace cache key.

This avoids a dangerous sequence where Client A lists Project Phoenix tools, Client B switches an ambient user preference to Project Soma, and Client A's next tool call accidentally executes with Soma credentials.

## Code Mode

Code Mode inherits the owning `BoundAccessContext`. Caller-provided `_meta` cannot select or replace authorization facts. Serialization carries only an opaque server-owned context reference or integrity-protected internal envelope. Nested calls, tasks, MCP Apps, pagination, notifications, and cancellation retain the same binding.

Code Mode inherits the Project context of its owning Labby MCP/API execution context.

Search, describe, snippets, batch calls, nested upstream calls, and MCP Apps all operate inside the same EffectiveWorkspace projection unless a future explicitly authorized cross-Project administrative operation is designed.

An upstream tool result, MCP App, snippet, or nested call cannot mutate the Project context.

Dynamic upstream discovery is intersected with the already bound Project workspace. Code Mode search never sees a union of every Project the Principal could potentially access.

## Protected routes and Loadouts

Current protected gateway routes and GatewayLoadoutConfig already narrow runtime capability exposure. Project context composes with this existing layer.

A Project-bound route resolves:

- the authenticated Principal;
- exact Project;
- Project/Group/Organization Assignments;
- assigned/current Loadout projection;
- current gateway catalog;
- Project-specific runtime bindings.

Each layer can only narrow. A Project assignment cannot re-enable a capability disabled by the route/Loadout/current gateway exposure.

## Artifact transfer

Add to My Labby / Send to requests carry an explicit source scope. If the source is Project-scoped, the exact Project is included in transfer authorization and audit evidence.

The destination's Personal workspace is a separate destination context. Possessing a mirrored/forked Artifact locally does not mutate the user's currently selected source Project.

## Background/follow operations

Managed Artifact follow/subscription jobs store the exact source Organization/Project/scope identity needed to reauthorize each update.

They MUST NOT consult a user's current UI/CLI Project preference at execution time.

If the source Project is deleted, disabled, or membership is revoked, the follow operation fails closed and updates managed mirror state accordingly.

## Cache keys

Milestone 1 does not cache authorization decisions. Pagination/task/resume state still binds to the exact context fingerprint and cannot be replayed under another Principal or Project.

Project-sensitive cache entries include at least:

- principal_id;
- organization_id;
- project_id;
- policy_epoch/project_policy_epoch as applicable;
- gateway_catalog_generation;
- any target/source revision required for correctness.

A cached Project A workspace can never satisfy a Project B lookup.

MCP/session-scoped caches additionally bind to the route/session context when needed to prevent accidental reuse across sessions with different exposure policy.

## Runtime bindings

Project ID is a required input to runtime-binding selection for Project-scoped capabilities.

Given the same upstream/tool name in Projects A and B, the dispatcher selects only a binding whose Project ID matches the current context. Missing or ambiguous bindings fail closed. There is no "closest" or organization-wide credential fallback unless an explicit Organization-scoped binding policy is added and authorized by contract.

## Revocation and long-lived sessions

Revocation committed before the final dispatch authorization check denies the next external side effect. Already-started one-step effects use explicit start-authorized semantics. Multi-step operations re-authorize before each new independently avoidable side effect. Logout, credential revocation, Principal suspension, and membership removal reject subsequent use of the binding.

Tests cover revoke/check/dispatch races, reconnect/resume, context substitution, nested `_meta` forgery, task continuation, and two-session Project isolation.

A Project-bound session does not freeze authorization forever.

Direct actions validate that the cached workspace/decision still matches the current policy epoch/catalog generation before crossing sensitive runtime boundaries. Revoking membership, Grant, Assignment, or runtime-binding policy invalidates future actions in an already-open MCP/web session.

The session may remain connected, but its prior catalog is not authority.

## Audit

Project-sensitive audit records contain the exact Project ID or safe fingerprint required by redaction policy. They do not log a human-facing Project name when that would leak a hidden scope.

Context-change events should be auditable where they affect long-lived sessions, especially MCP/agent sessions and runtime binding selection.

## Required tests

1. Two concurrent MCP sessions for the same Principal can bind different Projects without catalog or credential bleed.
2. Changing a web/CLI Project preference does not mutate an existing MCP session.
3. A Project-required operation with no bound Project fails instead of selecting the caller's first/most privileged Project.
4. A foreign/cross-Organization Project ID is rejected without enumeration.
5. Code Mode cannot switch Project through nested call parameters or an MCP App payload.
6. Project A cached EffectiveWorkspace is never reused for Project B.
7. Revoking Project A membership prevents the next action in an already-open Project A MCP session.
8. The same upstream name in two Projects selects only the matching Project RuntimeBinding.
9. A background follow job reauthorizes against its stored source Project, not the user's current UI Project.
10. Switching Projects creates a distinct audit/cache context and does not mutate Personal workspace ownership/state.
