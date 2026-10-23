# labby-gateway

`labby-gateway` is Labby's surface-neutral MCP gateway runtime. It connects to
upstream servers, discovers and publishes their capabilities, routes calls,
maintains OAuth-scoped peers, and binds gateway capabilities to Code Mode.
Developers embedding or changing the runtime start here; operator setup belongs
in the [gateway guide](../../docs/services/GATEWAY.md) and
[upstream contract](../../docs/services/UPSTREAM.md).

This crate is a workspace library with `publish = false`. It does not own a CLI,
an HTTP listener, product MCP handlers, or the web UI. The product adapts it in
[`labby/src/dispatch/upstream.rs`](../labby/src/dispatch/upstream.rs) and supplies
host-specific configuration, caller authority, and service registration.

## Architecture and dependency boundaries

The two main runtime layers are:

| Layer | Responsibility | Entry point |
| --- | --- | --- |
| Upstream pool | Connections, capability catalogs, exposure, relays, cancellation, health, and recovery | [`UpstreamPool`](src/upstream/pool.rs) |
| Gateway manager | Configuration transactions, pool publication, imports, routes, loadouts, OAuth coordination, and Code Mode integration | [`GatewayManager`](src/gateway/manager.rs) |

`labby-runtime` owns shared configuration and error contracts. `labby-auth`
supplies upstream OAuth caches, managers, and encrypted persistence.
`labby-codemode` owns the host-neutral JavaScript kernel and runner lifecycle.
`labby-openapi` supplies the optional OpenAPI catalog and dispatch provider.
`labby-primitives` and `labby-apis` provide shared policies and SDK contracts.
The [manifest](Cargo.toml) owns dependency and feature definitions.

Keep product registry types and action dispatch pointers outside this crate.
The runtime accepts injected contracts rather than constructing the product's
default registry. See the [architecture guide](../../docs/ARCH.md).

## Public entry points and host injection

[`gateway`](src/gateway.rs) exports the manager, gateway dispatch functions,
configuration-store contract, service-publication types, and execution-loadout
contracts. [`upstream`](src/upstream.rs) exposes the pool, transport helpers,
capability types, and upstream error types. Prefer explicit manager injection
through `dispatch_with_manager` or `dispatch_with_manager_scoped` when the host
already owns a manager; the module also has process-level installation helpers.

Production hosts construct a manager with `GatewayManager::from_config` and
[`GatewayManagerConfig`](src/gateway/manager/core.rs). The factory returns a
`Result` so bootstrap failures reach the host. Its main seams are:

| Contract | Host responsibility |
| --- | --- |
| [`GatewayConfigStore`](src/gateway/config_store.rs) | Persist gateway sections without losing unrelated host configuration; resolve public URLs; write credentials; synchronize the process Code Mode flag |
| [`GatewayServiceRegistry`](src/gateway/service_registry.rs) | Publish immutable built-in service descriptions, actions, safety metadata, and registration handles |
| [`InProcessConnector`](src/upstream/pool.rs) and [`InProcessServiceRegistry`](src/registry.rs) | Turn opaque host service handles into live MCP peers without importing product dispatch into the pool |
| `GatewayRuntimeHandle` | Share the published pool and its publication generation |
| `GatewayOauthConfig` | Supply the upstream OAuth cache, managers, encrypted store, key, and redirect URI |

Usage recording, durable task routes, resource registration, and the execution
capability provider are optional factory inputs. Builders attach Code Mode
skill, artifact-search, personal-OAuth, OpenAPI, Core-provider, runner-spawn, and
step-journal integrations. Providers do not confer implicit caller authority.

The service registry must remain immutable after publication. Replacement
publishes a new generation; hidden interior mutation bypasses freshness checks.
The connector recovers the host registration payload through the type-erased
[`InProcessService`](src/registry.rs) handle.

## Host integration

A production integration follows this sequence:

1. Load the host's configuration and establish its caller/authentication policy.
   Provide a writable configuration directory and implement `GatewayConfigStore`
   over the host's existing persistence authority.
2. Build the service registry and optional in-process connector, OAuth runtime,
   and stores. Create `GatewayRuntimeHandle::default()` and pass it with
   `GatewayManagerConfig` to `GatewayManager::from_config`.
3. Attach any additional providers and the catalog-change notifier before
   seeding. Call `try_seed_config` with the validated `GatewayConfig`; propagate
   its error rather than treating failed initialization as a usable manager.
4. Adapt incoming requests to the manager with the verified caller and scope.
   Code Mode's `CodeModeHost` implementation performs bounded cold discovery
   when an authorized request needs an upstream that is not warm yet.
5. Preserve publication identities when listing and later invoking capabilities.
   The manager's `execute_published_tool_exact`, `execute_published_prompt_exact`,
   and `execute_published_resource_exact` paths validate the published target.
6. Keep the Tokio runtime alive for active peers and cleanup. During shutdown,
   stop new work, shut down the Code Mode runner pool, and drain owned upstream
   pools through their lifecycle methods.

The [manager tests](src/gateway/manager/tests.rs) contain construction fixtures.
[Code Mode integration tests](src/gateway/manager/tests/code_mode.rs) exercise
cold discovery, exposure, caller scope, and resource reads against mock upstreams.
These are executable examples; README prose and code fences are not automatically
compiled or executed.

For a pool-only integration, `UpstreamPool::new()` accepts builders for the
connector, OAuth cache, request/relay timeouts, concurrency, and stores.
`discover_all` connects non-OAuth upstreams; subject-aware variants select an
explicit OAuth identity. Discovery records per-upstream outcomes, so inspect
health and cached summaries alongside `healthy_tools()` rather than treating an
empty list as proof that every upstream has no tools.

## Transports and runtime lifecycle

[`connect.rs`](src/upstream/pool/connect.rs) selects Streamable HTTP, WebSocket,
stdio, or Unix-socket transport from `UpstreamConfig`. HTTP and Unix sockets
share rmcp's Streamable HTTP machinery and gateway response/header policies.
WebSocket uses the gateway's WebSocket transport and rejects upstream OAuth
configuration; it is not interchangeable with the HTTP OAuth path.

Stdio starts a local child process and owns process-group/job cleanup,
bounded framing, and stderr diagnostics. Its transport is the single observer
of child exit. Lifecycle compatibility recovery uses typed protocol errors;
ordinary stderr text or child death alone does not authorize downgrading and
respawning. In-process peers use the injected connector rather than a listener.

The manager seeds lazy upstream entries and coordinates replacement pools.
Reload publishes coherent configuration, route, and pool state before draining
the displaced runtime. Per-upstream connection gates, capability-specific
health, circuit breaking, and optional reconnect probes prevent one failing
upstream from becoming fleet-wide execution authority or a global refresh lock.

## Bounded catalogs and exposure

Catalog reads use [`catalog_pagination.rs`](src/upstream/pool/catalog_pagination.rs),
which limits pages, items, cursor size, serialized bytes, and total elapsed time.
Reaching the per-traversal item cap returns the items collected so far and logs
the cap. Repeated cursors, shared item/byte budget exhaustion, oversized pages,
and page, byte, cursor, or deadline limits return errors. Never replace these
helpers with rmcp's unbounded `Peer::list_all_*` conveniences. Source constants and
runtime configuration own the limits; a cap applies at its particular transport,
page, upstream, or merged-publication boundary.

Tools, resources, resource templates, and prompts have independent publication
generations. [`catalog_publication.rs`](src/upstream/pool/catalog_publication.rs)
constructs immutable routable projections and withholds invalid source rows
according to the owning projection's admission rules. Subject-private, UI, and
synthetic capabilities do not become ordinary fleet catalog rows. Preserve
native `ui://` identities and upstream annotations.

`expose_tools`, `expose_resources`, and `expose_prompts` apply both to discovery
and direct execution/read/get/completion paths. Operator inspection can retain
hidden rows for administration; that snapshot does not authorize execution.
Resource reads and listing use bounded cache warm-up, and `list_changed`
refreshes coalesce per upstream before downstream notification. Refresh work for
one upstream must not delay unrelated upstreams.

## OAuth identity and invalidation

Subject-less discovery skips OAuth upstreams. Authenticated peers, optional
catalogs, relay sessions, and retained task routes preserve their subject and
owner provenance. Relay cache identity also includes the downstream session and
capability fingerprint, allowing in-flight generations to coexist safely.

Credential replacement, refresh, clear, and shared-provider revocation use
[`oauth_invalidation.rs`](src/upstream/pool/oauth_invalidation.rs). Invalidation
advances lifecycle epochs and detaches affected peers and task routes while
holding the credential writer. Transport shutdown proceeds after that writer
is released. Asynchronous connection/catalog publication checks the captured
epoch before publishing, so late discovery cannot restore invalidated state.
Never log raw subjects or credentials, or reuse a shared catalog as proof of a
caller's access. See the [access contract](../../docs/services/ACCESS.md).

## Configuration, loadouts, and state ownership

[`GatewayConfig`](../labby-runtime/src/gateway_config.rs) defines gateway-owned
configuration, including upstreams, mounted loadouts, routes, virtual servers,
preferences, and Code Mode settings. Host loading owns environment and file
precedence. Manager mutations serialize, validate, persist through the injected
store, and coordinate live publication and rollback; import discovery preserves
operator-owned configuration and import tombstones.

Configured gateway loadouts define reusable mounted capability projections.
Changes that affect an enabled route can require staged desired state and a
restart; the manager exposes desired-versus-runtime state explicitly.
[`execution_loadout`](src/gateway/execution_loadout.rs) is a separate contract:
revisioned per-turn selections scoped to an explicit principal and tenant,
resolved against a host-authoritative immutable capability snapshot. Selection
does not grant permission beyond that snapshot or replace invocation-time checks.

Durable state includes host configuration/credentials, execution loadouts in
`execution-loadouts.json` beside the configuration, and production agent
execution records. Usage, task-route metadata, and step journals persist only
when their respective stores are wired. Execution-loadout writes distinguish a
failure before publication from a durability failure after publication; callers
must preserve that distinction when reporting or retrying a mutation.

Live peers, health, published catalog generations, route indexes, rendered
catalogs, embeddings, and in-memory Code Mode history are runtime/derived state.
The one-shot catalog cache helps discovery and never grants execution authority.
Configuration definitions and their derived projections must retain one owner.

## Code Mode host boundary

[`code_mode_host.rs`](src/gateway/code_mode/code_mode_host.rs) implements
`labby_codemode::CodeModeHost` for `GatewayManager`. It supplies scoped discovery,
tool calls, resources, OAuth selection, and injected provider capabilities.
The gateway owns catalog projection, search/embedding caches, upstream error
translation, and optional step-journal integration.

`labby-codemode` owns JavaScript execution, Javy/QuickJS runner mechanics, warm
runner pools, broker protocol, result shaping, artifacts, and snippets. A host
using Code Mode must supply an executable/runner configuration supported by that
crate; constructing a gateway manager does not turn an arbitrary executable into
a runner. See [Code Mode](../../docs/dev/CODE_MODE.md) and the
[tool-error contract](../../docs/contracts/code-mode-tool-errors.md).

## Errors, cancellation, and security

[`CapabilityCallError`](src/upstream/pool/capability_call.rs) separates MCP
application errors from transport, protocol, timeout, cancellation, and budget
failures. A valid MCP error proves a live connection and does not count as a
transport breaker failure. Preserve typed causes, recovery data, `side_effects`,
and `input_required` until the owning surface maps them.

Caller-attributed RPCs enter the per-upstream concurrency bulkhead and use the
downstream cancellation token. Already-cancelled calls must not dispatch.
Pooled and relay paths propagate cancellation with bounded delivery/cleanup;
dropping only a local response future does not stop remote work. Cancellation
is not a breaker failure, while an actual upstream timeout remains one.
Typed `HEADER_MISMATCH` recovery refreshes the exact peer schema and permits one
replay under the original deadline, not a general retry of arbitrary failures.

Stdio configuration is trusted operator input but still passes command,
argument, environment, and sandbox guards. The basename allowlist does not
verify executable identity. Preserve caller, tenant, loadout, OAuth, and retained
result isolation, as well as response budgets, redirect policy, SSRF preflight,
and secret redaction. DNS preflight does not pin a later connection against
rebinding. The [error guide](../../docs/dev/ERRORS.md) explains surface mapping.

## Features, platforms, and verification

The `skills` feature enables upstream Agent Skills aggregation. Upstreams must
also opt in through `proxy_skills`; `expose_skills` applies on every read of the
subject-scoped validated cache. The `testkit` feature exposes inspection helpers
and a filesystem-backed `FsGatewayConfigStore` for downstream tests. Keep it in
development/test builds; production hosts inject their own persistence store.

Use the workspace [Rust toolchain](../../rust-toolchain.toml) and
[testing guide](../../docs/dev/TESTING.md). Tokio process/network support and
the upstream executable are runtime prerequisites for their respective paths.
Unix sockets require Unix; abstract socket names require Linux. Windows process
containment uses `labby-winjob`. The required host-service stdio filesystem
sandbox is Linux-specific and fails closed on unsupported platforms.

From the repository root, focused package checks are:

```bash
cargo nextest run -p labby-gateway --all-features
cargo clippy -p labby-gateway --all-features --all-targets -- -D warnings
```

Use focused test filters while working, including the exposure, OAuth epoch,
list-change, cancellation, transport, and publication regressions under
[`upstream/pool`](src/upstream/pool.rs). Nextest excludes doctests; follow the
root `just rustdoc-check` contract when Rustdoc or doctests change. Module moves
also require `just module-reachability`. For implementation rules and deeper
rationale, read [crate instructions](AGENTS.md),
[gateway instructions](src/gateway/AGENTS.md),
[upstream instructions](src/upstream/AGENTS.md), and
[upstream maintenance notes](../../docs/dev/UPSTREAM_INTERNALS.md).
