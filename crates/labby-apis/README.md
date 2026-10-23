# labby-apis

`labby-apis` provides reusable Rust SDK contracts, a shared HTTP client, health
probing, setup validation, and the remote Artifact control-plane client. It is
a library boundary for explicit configuration and typed results. Product CLI,
MCP, HTTP-server, presentation, authorization, and durable configuration flows
belong to their host adapters.

The crate has no `clap`, `rmcp`, `axum`, or `anyhow` dependency. It never selects
an installation, reads configuration files, or resolves credentials from the
ambient environment. Callers supply URLs, auth, paths, schema declarations, and
runtime inputs. See the [architecture](../../docs/ARCH.md) and
[crate instructions](AGENTS.md) for dependency rules.

## Modules and ownership

| Module | Responsibility | Entry point |
| --- | --- | --- |
| `core` | HTTP/auth/error/status contracts, shared metadata re-exports, and pure SSH-config parsing | [`core.rs`](src/core.rs) |
| `doctor` | Generic HTTP reachability probe, probe result, and compile-time metadata | [`doctor.rs`](src/doctor.rs) |
| `setup` | Stateless schema validation, first-run/draft data types, and compile-time metadata | [`setup.rs`](src/setup.rs) |
| `artifact_control` | Curated remote operations, schema compatibility checks, and upload transport | [`artifact_control.rs`](src/artifact_control.rs) |

Action, parameter, plugin, UI-schema, and static SSRF vocabulary lives in the
[`labby-primitives`](../labby-primitives/Cargo.toml) internal leaf crate and is
re-exported from `core`. Edit the canonical types there rather than defining
another SDK copy. `ActionSpec.destructive` and `requires_admin` are independent
policy inputs; `returns` is a descriptive hint and `output_schema` supplies the
success-payload schema. The host owns enforcement and presentation.

[`ServiceClient`](src/core/traits.rs) exposes service identity and a health
method returning `impl Future<Output = Result<ServiceStatus, ApiError>> + Send`.
It uses concrete implementations rather than a `dyn ServiceClient` interface.
[`ServiceStatus`](src/core/status.rs) distinguishes unreachable, unauthorized,
and reachable-but-degraded outcomes. It is a data contract, not a scheduler or
an automatic health-check deadline.

## Shared HTTP client

[`HttpClient`](src/core/http.rs) is a cloneable `reqwest::Client` wrapper. It
injects [`Auth`](src/core/auth.rs), builds relative request paths, maps status
and decode failures to `ApiError`, and emits structured request events. The
public methods cover JSON requests, query parameters, text/bytes, form and
multipart bodies, GraphQL, and body-discarding operations.

`new` and `with_default_headers` install the Rustls ring provider when needed
and build a client with connect and overall request timeouts. The constructor
implementation owns their defaults. There is no SDK retry loop, backoff, rate
limiter, or caller authorization policy; a host decides whether an operation is
safe to repeat and applies its own bounded scheduling and cancellation.

`with_pinned_addresses` and `with_pinned_addresses_and_headers` bind the
configured origin to caller-approved IP addresses and disable redirects. They
reject missing hosts, unusable ports, and empty address sets. The caller still
owns endpoint validation, address allow/deny policy, and credential selection;
the generic constructor does not apply the static SSRF helpers automatically.
Use the [remote-target contract](../../docs/design/REMOTE_GATEWAY_TARGET.md)
when adapting an operator-selected remote authority.

Request helpers accept paths relative to the configured base. They reject
absolute `http://` and `https://` request paths so a supplied path cannot select
another authority that receives the configured auth. Encode variable path
segments with `HttpClient::encode_path_segment`; build query parameters through
the query helpers. Segment encoding is not a filesystem/path-traversal policy.

Choose a `*_bounded` JSON/body helper when the caller requires a response byte
budget. Those helpers enforce the supplied budget while reading, before JSON
decode, including error responses. Ordinary JSON, text, and byte helpers do not
inherit a universal size limit. Upload callers also own the request-body budget.

`Auth` supports no auth, a configurable API-key header, Token/Bearer schemes,
Basic auth, and session cookies. Its `Debug` implementation redacts credential
values while retaining non-secret context such as header names and usernames.
Request events record method, host, path, timing, status, and stable error kind;
the error logger excludes provider response bodies and credential headers.
Returned `ApiError` values can still carry provider bodies or transport details,
so callers must apply their surface's redaction policy before exposing them.

## Minimal HTTP integration

This helper runs inside the host's Tokio runtime. The caller supplies an
approved base URL, resolved auth, relative path, and response budget:

```rust
use labby_apis::core::{ApiError, Auth, HttpClient};
use serde_json::Value;

pub async fn fetch_json(
    base_url: &str,
    auth: Auth,
    path: &str,
    max_response_bytes: usize,
) -> Result<Value, ApiError> {
    let http = HttpClient::new(base_url, auth)?;
    http.get_json_bounded(path, max_response_bytes).await
}
```

Reuse a cloned client for repeated calls rather than rebuilding its connection
pool. The [HTTP tests](src/core/http.rs) exercise relative-path rejection,
segment encoding, and provider-error log redaction. The example is explanatory;
README code fences are not included in Rustdoc doctests.

## Doctor and setup contracts

[`DoctorClient::new`](src/doctor/client.rs) accepts an explicit base URL with
`Auth::None`. `probe()` performs `GET /` and translates its HTTP result into
`ServiceStatus`: authentication failures are reachable but unauthorized,
transport failures are unreachable, and other HTTP/decode failures are
reachable but degraded. Probe failures are represented in the returned status.
It does not discover service configuration, run system commands, or implement
`ServiceClient`. The product's broader audit lives in
[`dispatch/doctor`](../labby/src/dispatch/doctor.rs); see the
[doctor guide](../../docs/services/DOCTOR.md).

[`SetupClient`](src/setup/client.rs) is a stateless validation helper.
`validate_against_ui_schema` checks required/length constraints and field kinds,
including URL schemes, booleans, enum membership, and lexical path restrictions.
Filesystem containment and safe-root resolution remain host responsibilities.
The [setup types](src/setup/types.rs) describe state, snapshots, draft entries,
and commit outcomes; constructing them does not read or write a draft.
`SECRET_SENTINEL` represents values the product redacts before returning draft
data. The [setup guide](../../docs/services/SETUP.md) owns the operator flow.

## Artifact control-plane contracts

[`ArtifactControlClient`](src/artifact_control.rs) receives an already-configured
`HttpClient`. `Operation` seals the supported operation names; callers cannot
pass an arbitrary provider operation string. The implementation and its golden
fixture own the operation list rather than a second README inventory.

Each `execute` fetches the bounded `/api/operations` catalog and checks the
selected operation before posting to its encoded operation route. Compatibility
requires the expected name/version, object input/output schema types, and
agreement between the locally supported input-schema fingerprint, the declared
fingerprint, and the fingerprint computed from the served input schema.
An absent or incompatible operation returns an error before execution.
`schema_fingerprint` uses canonical JSON hashing and rejects unsupported numeric
forms. It fingerprints the input schema, not the entire output contract.

`execute_with_headers` forwards request-local delegation headers in addition to
the configured client auth. `upload_with_headers` streams an owned request body
to an existing principal-bound upload slot and bounds the decoded response.
The SDK does not mint delegation tokens, establish the principal, authorize the
operation, or reserve the slot. Those decisions belong to the
[`product Depot adapter`](../labby/src/dispatch/depot.rs), which supplies fresh
delegation and server-held credentials. Upload identifiers are encoded as path
segments; an identifier alone is not authorization.

The [Artifact control tests](src/artifact_control.rs) cover curated-operation
mapping, schema/fingerprint disagreement, unsupported contracts, server auth,
and response budgets. [Depot providers](../../docs/services/DEPOT_PROVIDERS.md)
describes the product's provider boundary.

## Features and verification

The [manifest](Cargo.toml) declares empty `default` and `all` features. `all`
is a compatibility aggregate, and `test-utils` is a reserved marker. Enabling
them does not install another service SDK, register a product action, or enable
ambient configuration reads. Doctor and setup expose `META` declarations for
consumers; runtime registration remains a host decision.

Use the workspace [Rust pin](../../rust-toolchain.toml) and
[testing guide](../../docs/dev/TESTING.md). From the repository root:

```bash
cargo nextest run -p labby-apis --all-features
cargo check -p labby-apis --no-default-features
cargo check -p labby-apis --no-default-features --features all
```

HTTP/control-plane tests use local Wiremock servers rather than live provider
credentials. Nextest excludes doctests; use the root `just rustdoc-check`
contract when changing Rustdoc or its examples. Preserve the stable
[`ApiError::kind()` taxonomy](src/core/error.rs) and consult the
[error mapping guide](../../docs/dev/ERRORS.md) before changing surface semantics.
