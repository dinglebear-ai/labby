# labby-openapi

`labby-openapi` turns operator-selected JSON OpenAPI documents into allowlisted
operations for Code Mode's `openapi` provider. It owns operation extraction,
bounded spec loading, an immutable registry, and hardened outbound HTTP.
The host owns configuration discovery, caller authorization, and OAuth lookup.
See [Code Mode](../../docs/dev/CODE_MODE.md) for the product-facing provider.

This workspace library has `publish = false` and no declared Cargo features.
It depends on `labby-runtime` for shared error contracts and `labby-primitives`
for canonical SSRF policy. It does not depend on `labby-codemode` or
`labby-gateway`; those consumers adapt its public APIs. Code Mode carries the
registry/client through its host contracts, while this crate owns the OpenAPI
HTTP implementation. The [manifest](Cargo.toml) owns dependency details.

## Public APIs and configuration

| Module | Public contract |
| --- | --- |
| [`config`](src/config.rs) | `OpenApiProviderConfig`, `OpenApiSpecConfig`, `SpecSource`, `OpenApiCredential`, and reserved labels |
| [`convert`](src/convert.rs) | `convert_spec` and `OperationDescriptor` for JSON-to-operation extraction |
| [`registry`](src/registry.rs) | `OpenApiRegistry`, `SpecEntry`, and resolved `OperationHandle` values |
| [`dispatch`](src/dispatch.rs) | `dispatch_openapi_call` and its caller-credential variant |
| [`http`](src/http.rs) | Hardened client construction, capped remote fetch, and operation execution |
| [`error`](src/error.rs) and [`ssrf`](src/ssrf.rs) | Scrubbed `OpenApiError` mapping and URL preflight |

An `OpenApiSpecConfig` explicitly supplies a label, URL or filesystem source,
mandatory `base_url`, raw-operation-ID allowlist, and optional static credential
or `oauth_upstream` reference. There is no base URL inferred from `servers[]`.
The spec location and operation base URL are separate authorities and each
remote URL passes the owning HTTPS checks.

The product's [`load_openapi_provider_config`](../labby/src/config.rs) validates
unique/unambiguous labels, reserved namespaces, an explicit parseable base URL,
exactly one spec source, and mutually exclusive static/OAuth auth. It resolves
static credentials through the host's credential environment. Embedders that
construct these Rust types directly must preserve those preconditions; the
registry is not a replacement for the product configuration loader.

Configuration types do not discover files or read the process environment.
Registry loading deliberately reads the explicitly selected spec file or URL.
Loaded registries are cheap to clone and have no background refresh or TTL;
the host constructs and publishes replacements when configuration changes.

## Operation extraction

`convert_spec(label, spec_json, allowed)` parses with `serde_json`. It requires
a top-level `paths` object, visits supported HTTP method entries, and extracts
only the raw `operationId`, method, and path template. An operation without an
ID or without an exact allowlist match is omitted. An empty allowlist admits
no operations. IDs remain raw keys, including their case and punctuation.

This is a focused dispatch extractor, not a complete OpenAPI/schema interpreter.
It does not resolve `$ref`, interpret `servers[]`, derive complete request
schemas, or execute a third-party OpenAPI client. YAML is not a supported input.
Parameters follow the gateway's flat-object dispatch convention described below,
rather than OpenAPI's full serialization and security-scheme vocabulary.

This reduced fixture matches the [converter tests](src/convert.rs):

```rust
use labby_openapi::{convert::convert_spec, OpenApiError};

fn allowed_operations() -> Result<(), OpenApiError> {
    let spec = r#"{"paths":{"/users/{id}":{
        "get":{"operationId":"getUser"}
    }}}"#;
    let operations = convert_spec("vendor", spec, &["getUser".into()])?;
    assert_eq!(operations[0].operation_id, "getUser");
    assert_eq!(operations[0].path_template, "/users/{id}");
    Ok(())
}
```

For an executable dispatch, construct `OpenApiProviderConfig`, call
`OpenApiRegistry::load(config, per_spec_timeout).await`, report `warnings()`, and
build the fallible `http::build_dispatch_client()`. Reuse that registry/client
pair with `dispatch_openapi_call(&registry, &client, label, operation_id, params)`.
The public dispatch key exposed by Code Mode is
`openapi::<label>.<operationId>`; Rust dispatch receives label and ID separately.
README code fences are explanatory and are not Rustdoc doctests.

## Bounded loading and degraded state

`OpenApiRegistry::load` loads a bounded set of configured specs concurrently,
with a caller-supplied deadline for each spec. URL fetches and regular-file reads
have a byte cap before JSON parsing. Local loading checks for a regular file
before and after opening; Unix also uses nonblocking open to avoid a FIFO
replacement race. Paths are explicit operator input, not a directory scanner.

A failed fetch, rejected base URL, malformed document, or timeout omits that spec
and records a scrubbed warning. Extra specs or operations beyond their retention
bounds produce warnings. A valid spec with no matching operations remains loaded
with an empty operation map and a warning; it is not the same as an omitted spec.
Inspect `labels()`, `is_empty()`, `warnings()`, and `operation()` when reporting
readiness. A returned registry does not imply that every configured spec loaded.

The [registry constants](src/registry.rs) own spec-count, operation-count, and
document-size bounds. The [HTTP implementation](src/http.rs) owns DNS,
connection/request deadlines and operation-response limits. These are separate
budgets; a load timeout is not a promise about a later operation's duration.

## Dispatch parameters and credentials

The registry resolves the exact label and raw operation ID before any request.
Unknown labels return `UnknownInstance`; missing or excluded IDs return
`UnknownOperation`. Pass a JSON object whose path-template names appear as
top-level keys. Path values must be nonempty string, number, or boolean scalars;
missing, null, composite, `.` and `..` values are rejected.

Consumed path keys do not appear again in the query/body. Remaining keys become
a JSON body for POST/PUT/PATCH and query pairs for the other supported methods.
An empty successful response becomes JSON `null`; nonempty success bodies must
decode as JSON. Non-success responses produce scrubbed errors rather than
passing upstream response bodies to the caller. There is no automatic retry.

Static credentials are either an API-key header or a Bearer token. They remain
server-side and are injected after the JavaScript sandbox boundary. For a spec
with `oauth_upstream`, the registry retains only that non-secret reference.
The host resolves the authenticated caller's credential and passes it to
`dispatch_openapi_call_with_credential`; dispatch clones the handle for that call
and requires the override. A missing override returns
`CallerCredentialRequired`. It does not persist a caller token in the registry.
For a static/no-OAuth handle, a supplied override does not replace its credential.

## Outbound security boundary

Both remote spec fetches and production operation execution use the hardened
client builder: HTTPS only, no redirects, no proxy, and explicit connect and
request timeouts. A supplied shared client does not bypass production pinning:
execution builds a fresh hardened client for the validated target address.

Load-time preflight uses [`labby_primitives::ssrf`](../labby-primitives/src/ssrf.rs)
to reject forbidden URL components, hosts, and IP literals. It does not resolve
DNS. Before each outbound request, bounded DNS resolution checks every returned
address against that same policy, then pins one approved address in the client.
After receiving a response, the runtime requires a peer address, checks that
its IP matches the pin, and applies the IP policy again. Pinning is the primary
rebinding defense; the peer check fails closed as a backstop.

Operation path templates must be paths, not alternate URL references. The
runtime rejects authority-style paths, backslashes, queries, and fragments;
encodes substituted values as opaque segments; and checks that the joined URL
retains the configured origin and base-path scope. Response collection stops at
the owning byte cap. Preserve these checks together: string host validation
alone does not establish safe execution.

## Errors, tests, and maintenance

`OpenApiError` carries fixed, scrubbed message templates and stable `kind()`
tags. Resolution failure is `ResolveFailed`, distinct from the forbidden-address
`RequestBlockedPrivateAddr`; send timeouts use `UpstreamTimeout`. Load/size/parse
variants use `config_error` for logging. The conversion to `ToolError` has its
own mapping, including `Sdk` kinds for timeout/internal failures; do not infer
wire behavior solely from a load-time log tag.

Never interpolate raw HTTP/parser errors, provider bodies, URLs with secret
components, or credentials into logs or surface errors. Credential `Debug`
implementations redact values, and dispatch emits structured context such as label,
operation, host, method, elapsed time, and stable outcome kind. The canary and
auth/path tests in [`tests_dispatch.rs`](src/tests_dispatch.rs) protect this
boundary; [`tests_config.rs`](src/tests_config.rs) and
[`tests_ssrf.rs`](src/tests_ssrf.rs) cover redaction and preflight.

Use the workspace [Rust pin](../../rust-toolchain.toml),
[testing guide](../../docs/dev/TESTING.md), and [crate instructions](AGENTS.md).
From the repository root, run focused checks with:

```bash
cargo nextest run -p labby-openapi
cargo clippy -p labby-openapi --all-targets -- -D warnings
```

[`live_petstore.rs`](tests/live_petstore.rs) contains an ignored public-network
load-and-dispatch smoke test and a non-network private-base rejection test.
Local Wiremock dispatch tests use a test-only bypass for loopback fixtures;
they do not prove the full production transport path. The ignored smoke test
uses the real guarded path and requires working public DNS/HTTPS:

```bash
cargo nextest run -p labby-openapi --run-ignored ignored-only -E 'test(petstore)'
```
