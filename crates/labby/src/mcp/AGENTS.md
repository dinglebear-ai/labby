# mcp/ — MCP protocol surface

This directory adapts shared product dispatch and reusable gateway/runtime behavior to the MCP protocol. It owns protocol dispatch, envelopes, resources, elicitation, and catalog projection, not product operation semantics. The pure setup/doctor SDK is one dependency, not the universal backend for every MCP service.

## One descriptor builder, two consumers

`PermanentToolRegistry` in `permanent_tools.rs` is the sole construction site
for Labby-owned descriptors. These two consumers must expose identical metadata:

- `handlers_tools.rs::list_tools_impl` — what goes on the wire.
- `peer_contract.rs::visible_tool_descriptors` — what feeds
  `descriptor_contract_hash`, which drives `tools/list_changed`.

Divergence between them is silent: the hash says the catalog changed when the
wire content did not, or vice versa. Do not edit one site alone. Route descriptor
construction through a shared per-tool builder and change that.

Two asymmetries are deliberate and must be preserved: `list_tools_impl`
paginates and can early-break (`tools.finished()`), while
`visible_tool_descriptors` always builds the full list. Any equality assertion
between them therefore only holds below the page cap.

Note the visibility gates are spelled differently at the two sites
(`server_logs_app_visible` vs `self.audience.admin_apps_visible`) but resolve to
the same predicate on the live path, because production `PeerContract`s are built
by `peer_contract_for_request`. `PeerCatalogAudience::default()` hardcodes
`admin_apps_visible: true` and must never reach a real `tools/list` response.

## MCP tool annotations

MCP `ToolAnnotations` are **per tool**; `ActionSpec.destructive` is **per action**;
and one Labby tool fronts a whole service. A tool-level hint is therefore the
least-safe **union** of that service's actions — `setup` advertising
`destructiveHint: true` means "at least one action here is destructive", not
"this call is destructive".

Do not confuse the two mechanisms. `ActionSpec.destructive` is the authorization
gate for local dispatch (elicitation, CLI confirmation). `ToolAnnotations` are
advertised metadata — but not inert: in a labby → labby chain the next hop
derives its own `destructive` judgement from them, so accuracy is load-bearing.

Upstream annotations pass through **verbatim**. Never normalize, enrich, or
overwrite them.

Maintenance rule: annotations must stay a pure function of the static catalogs —
never per-peer, per-request, or time-dependent, or the contract hash churns.
Do not memoize them in a process-global keyed by service name; different
registries (`build_default_registry`, `build_docs_registry`, test registries)
produce different service sets. A new service needs a reviewed hint row; a
`readOnly` claim asserts every action is non-mutating, which is stronger than
"has no destructive actions" and cannot be derived from `ActionSpec`.

Current annotation and safety-hint behavior is documented in `docs/surfaces/MCP.md`.

## Router and atomic projections

The registry defaults to router projection: a service tool uses `action` +
`params`. `ToolProjectionMode` also models atomic and combined publication.
Atomic names and flat per-action schemas come from the same `ActionSpec` via
`PermanentToolRegistry`; do not maintain a second authorization or schema
catalog. Keep listing, exact resolution, output schemas, and peer-contract
hashing synchronized for every supported projection. Registration lives in
`crates/labby/src/registry.rs`; the MCP registry module is a compatibility alias.

## Dispatch pattern

For normal services, `dispatch/<service>/dispatch.rs` owns action routing, catalog, param validation, and client resolution. See `crates/labby/src/dispatch/AGENTS.md` for the required layout and templates.

`mcp/services/` is now an exception layer, not the default adapter surface. Keep a module there only when it owns MCP-specific behavior that cannot live in shared dispatch. Current examples:

- `fs` filters `fs.preview` out of MCP discovery and execution.
- Code Mode is registered directly in the MCP layer and bypasses both
  `dispatch/` and `mcp/services/`. The public surface is intentionally split:
  `codemode` has no static app descriptor but may return dynamic `_meta.ui`
  when an upstream call launches a nested MCP App. `codemode_ui` shares the
  same execution backend and owns the Code Mode inspector metadata. `mcp_app`
  is the always-available root-gateway control tool for the manager UI,
  inspector, Skill Library, Gateway Status, Server Logs, Add Server, and Settings surfaces; it
  supports per-app and `all` `status|enable|disable` operations. Its own manager
  UI is opt-in by default, while the text-only control tool remains available. App mutations require `lab:admin`, are gateway-scoped, and schedule
  coalesced
  `tools/list_changed` plus `resources/list_changed` notifications after the
  open tool turn drains. `server_logs` keeps its text/service capability when
  its UI is hidden; `codemode` likewise remains text-only and executable when
  only the inspector is disabled.
  Shared Code Mode execution belongs in the extracted gateway/Code Mode
  runtimes and product dispatch wiring, so the CLI does not route through MCP.
  Do not recreate the removed product-local `dispatch/gateway/code_mode.rs`.

**No product business logic in `mcp/`.** Transport adaptation stays here; operation semantics, upstream HTTP, validation, and retry policy belong in shared dispatch or the owning extracted runtime. Do not create an arbitrary SDK service module merely to move code out of this adapter.

## Structured error envelopes

`ToolError` is defined in `labby-runtime/src/error.rs` and re-exported through product dispatch and `envelope.rs`. Preserve its structured fields across MCP/API/CLI. The abbreviated core fields below are examples, not complete current wire payloads: real errors also carry the agent-error version, origin, recovery, and side-effect metadata, with the appropriate adapter envelope.

```jsonc
{ "kind": "missing_param", "message": "missing required parameter `query`", "param": "query" }
{ "kind": "unknown_action", "message": "...", "valid": ["movie.list", ...], "hint": null }
{ "kind": "auth_failed",    "message": "authentication failed" }   // SDK pass-through
```

Dispatcher-layer kinds:

| `kind` | When |
|--------|------|
| `unknown_action` | action not in the service's action table. Include `valid: [...]` and fuzzy `hint`. |
| `unknown_subaction` | subaction segment invalid. |
| `missing_param` | required param absent. Include `param` name. |
| `invalid_param` | param present but wrong type/value. |
| `unknown_instance` | multi-instance label not found. Include `valid: [...]`. |

SDK-layer kinds pass through from `ApiError::kind()` via `From<SdkError> for ToolError`: `auth_failed`, `not_found`, `rate_limited`, `validation_failed`, `network_error`, `server_error`, `decode_error`, `internal_error`.

### Serialization contract

`ToolError` uses a **custom `Serialize`** (not `#[derive(Serialize)]`) so that the `Sdk` variant promotes its `sdk_kind` field to the top-level `kind` field. The result is byte-identical across MCP and HTTP — never `{"kind":"sdk","sdk_kind":"auth_failed"}`.

- `Display` delegates to `serde_json::to_string(&self)` — output is always valid JSON.
- The HTTP adapter in `api/error.rs` derives status from the shared kind and preserves refined metadata; do not invent a transport-specific error taxonomy.
- Envelope tests and `labby-runtime/src/error.rs` / `agent_error.rs` tests lock in serialization and metadata preservation.

### Wiring per service

Each service dispatcher must:
1. Return `Result<Value, ToolError>` (not `anyhow::Result`).
2. Implement `From<ServiceError> for ToolError` mapping via `ApiError::kind()`.
3. Use `ToolError::MissingParam` / `UnknownAction` for dispatcher-layer errors.
4. Never use `anyhow::bail!` or `anyhow::anyhow!` inside a dispatch function.

## Elicitation for destructive ops

When an action's `ActionSpec.destructive == true`, the 2026-07-28 protocol
handler **must** return an MRTR `input_required` result containing form
elicitation in `inputRequests`. The client retries the original request with
`inputResponses`; do not send an in-flight `elicitation/create` RPC. Labby also
issues the protocol-standard opaque `requestState` and consumes it exactly once
on the retry. That state is server-owned, short-lived, and bound to the
canonical action, normalized params, authenticated caller, transport/session,
route, and action-catalog security metadata. Do not replace it with a custom
parameter, header, or client-asserted confirmation token.

When the MCP client does not support form elicitation, the dispatcher executes
normally. Do not add a `params.confirm`, `--yes`, header, or any other fake
destructive gate to the MCP path. Request params are payload, not
authorization. A declined or invalid elicitation retry returns
`confirmation_required`.

## Upstream MRTR forwarding

The above is Labby's own destructive-action elicitation. For an upstream MCP
server, the gateway preserves an `input_required` tool response for the
downstream client.
`mcp/call_tool_upstream.rs`, `mcp/handlers_prompts.rs`, and
`mcp/resource_proxy.rs` route proxied `call_tool`, `get_prompt`, and
`read_resource` requests through the corresponding `UpstreamPool::*_relayed`
method. Those methods use a dedicated connection served with
`RelayClientHandler` (see `crates/labby-gateway/src/upstream/pool/relay.rs`) instead of the
ordinary pooled connection. Both proxy branches do this automatically: the raw
branch passes `subject = None`, while the OAuth branch forwards
`oauth_subject`. The relay preserves request-scoped capabilities, MRTR input exchanges,
progress, and cancellation; it never fulfills client input inside Labby.

Relay connections are cached per `(upstream, session_id, subject, capability_fingerprint)`; capability generations may coexist while requests remain in flight. `session_id`
is minted once per `LabMcpServer` session (`next_relay_session_id()`) and passed
into `call_tool_relayed`; because each session has exactly one downstream agent
peer, it guarantees a cached relay connection is never reused across agents — so
the first relayed call in a session pays the connect cost and subsequent calls
reuse it, without risking misrouted elicitation. `subject` (the OAuth identity,
`None` on the raw branch) is part of the key so a connection authenticated as
one identity is never reused for a call made as another within the same session.
Use the current request capability snapshot, including an honest empty snapshot when metadata is absent; never borrow capability history from an earlier request.

**Deadline and cancellation.** A relayed request can block on a *human*
answering forwarded elicitation, so `call_tool`, `get_prompt`, and
`read_resource` are bounded by the pool's `relay_timeout`
(`upstream_relay_timeout_ms`, default 5 min) instead of the 30s
`upstream_request_timeout_ms` the pooled path uses — otherwise a confirmation
dialog left open would abort the upstream request. Each path also passes the
current request cancellation token and request ID into the relay. See
`config.rs` (`upstream_relay_timeout`) and
`crates/labby-gateway/src/upstream/pool/relay.rs`.

**Scope and capability exposure.** Relay handling covers proxied `call_tool`,
`get_prompt`, and `read_resource`; discovery operations such as `list_prompts`
and `list_resources` remain on the ordinary pool. The relay receives only the
capability snapshot attached to the current request. Even when modern request
metadata is absent, `forwardable_client_capabilities` supplies an honest empty
capability set and uses a relay connection so request-scoped progress and
cancellation still work. It never reuses capability history from an earlier
request or another downstream session.

## Built-in actions

Every tool automatically supports `help` and `schema` without the service declaring them. The dispatcher intercepts these before the action match.

## Catalog ownership

`crates/labby/src/catalog.rs::build_catalog` owns shared product catalog data
used by MCP discovery/resources and generated service/action inventories.
Extend that builder rather than duplicating catalog semantics. Operator CLI
help and completion come from the Clap command graph, not this MCP catalog;
changes to that graph require their own generated-help and completion checks.

## Resources

- `lab://<service>/actions` — per-service action catalog (name, description, destructive, params).
- `lab://catalog` — the full cross-service catalog.

Resources are read-only. Do not use them for mutations.

### `ui://` resources (MCP Apps / mcp-ui)

`read_resource_impl` splits the `ui://` namespace:

- `ui://lab/code-mode/*` — Lab's own Code Mode app resources, served locally
  from bundled HTML (`read_code_mode_app_resource_impl`). The app descriptors
  bind only to `codemode_ui`; disabling the app hides that tool and these
  resources, and direct reads fail as unknown. Labby-owned app UIs are opt-in;
  a disabled surface must not remain reachable through a cached URI.
- `ui://lab/apps/manage` — the opt-in UI for the always-available `mcp_app`
  control tool. Disabling this manager UI strips its tool metadata and resource
  but does not remove the text-only control tool needed to restore app surfaces.
- `ui://lab/gateway/add-server` — the admin-only Add Server app bound to the
  synthetic `add_server` tool. Its `test` and `create` callbacks delegate to
  `gateway.test` and `gateway.add`; do not duplicate gateway persistence logic.
- `ui://lab/gateway/status` — the admin-only live gateway connection and
  capability app bound to the synthetic `gateway_status` tool.
- `ui://lab/settings/editor` — the admin-only schema-backed settings app bound
  to the synthetic `settings` tool. Its callbacks delegate to the canonical
  `setup settings.*` dispatch actions; do not add a second configuration model.
- any other `ui://<upstream>/…` — an upstream MCP App widget resource. Owners may
  bind through standard `ui.resourceUri` or OpenAI `openai/outputTemplate`; the
  native URI is preserved. Under synthetic Code Mode, an owner passes through
  only when the route allows its upstream, `proxy_resources` is enabled, and the
  exact binding passes `expose_resources`. Callback-only metadata is accepted
  only when the same upstream has an exposed owner, ambiguous tool names are
  omitted, and destructive app tools require execute scope. OAuth app discovery
  and native `ui://` reads stay on the same subject-scoped cached connection; a
  subject resource denial must never fall through to a global peer. Keep
  `handlers_tools.rs` and `peer_contract.rs` on the single combined app-catalog
  helper so the advertised descriptor set and `tools/list_changed` hash cannot
  drift. See `resource_proxy.rs::read_upstream_ui_resource_impl`.

## Transport auth for fs

The `fs` service exposes workspace filesystem contents (`fs.list`,
`fs.preview`). The HTTP surface refuses to mount `/v1/fs` when
`LABBY_WEB_UI_AUTH_DISABLED=true` — see `api/router.rs` and the
corresponding gate in `cli/serve.rs`. The MCP surface has **no**
equivalent env-driven refusal: `fs` is registered unconditionally in
`registry.rs` whenever the `fs` feature is compiled in, regardless of
MCP transport auth posture.

Existing hard checks (enforced in code):

- Router: `/v1/fs` refuses to mount when
  `LABBY_WEB_UI_AUTH_DISABLED=true` (`api/router.rs`). This is the only
  enforcement that fires in the LABBY_WEB_UI_AUTH_DISABLED + LAN-bind
  scenario, because the bind guard below treats a configured bearer
  token as "auth configured" even though the `/v1` middleware has been
  bypassed.
- Bind: `cli/serve.rs` refuses to bind on a non-loopback address when
  no auth is configured at all (no bearer token, no OAuth). Does NOT
  fire when `LABBY_WEB_UI_AUTH_DISABLED=true` is paired with a token —
  that case relies on the router-level fs mount refusal above.

Operator-side (not enforced in code) — must be ensured before exposing
a server that has the `fs` feature enabled:

- `labby serve` (HTTP transport, the default): require
  `LABBY_MCP_HTTP_TOKEN` or `LABBY_AUTH_MODE=oauth`. Do not relax this
  while `fs` is feature-enabled.
- `labby mcp`: stdio has no transport-level auth. Ensure
  the process is not reachable by untrusted callers — do not expose it
  through a network proxy without front-side auth.

The asymmetry with `/v1/fs` is intentional: MCP registration is not
structured to fail or skip a single service at runtime, and stdio has
no single env var equivalent to `LABBY_WEB_UI_AUTH_DISABLED`. Promoting
this to a runtime invariant (e.g. a startup check that refuses to
register `fs` when MCP auth posture is not verified) is tracked as
follow-up work.
