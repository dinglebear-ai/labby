# labby-primitives

`labby-primitives` owns stable vocabulary and validation shared by Labby's SDK,
authentication, runtime, gateway, and Code Mode crates. It is a dependency leaf:
**zero internal workspace dependencies**, with third-party dependencies declared
in [Cargo.toml](Cargo.toml). Higher layers depend on these contracts; primitives
must not depend back on product routing, authentication services, or executors.

This workspace library has `publish = false`, no declared Cargo features, and
`#![forbid(unsafe_code)]`. It defines data and validation helpers rather
than a configuration loader, database, HTTP client, scheduler, or container
backend. [`lib.rs`](src/lib.rs) is the public module index; the
[architecture guide](../../docs/ARCH.md) explains the consumer boundaries.

## Public contract groups

| Modules | Responsibility |
| --- | --- |
| [`action`](src/action.rs), [`plugin`](src/plugin.rs), [`plugin_ui`](src/plugin_ui.rs) | Action/schema descriptors, service metadata, and const-friendly field/UI declarations |
| [`access`](src/access.rs), [`product_credential`](src/product_credential.rs) | Typed ownership/capability vocabulary, strict credential parsing, and injected verification/grant contracts |
| [`authority_projection`](src/authority_projection.rs) | Versioned projection envelopes, record operations, acknowledgments, and structural bounds |
| [`canonical_json`](src/canonical_json.rs), [`digest`](src/digest.rs) | Canonical encoding, fingerprints, and validated SHA-256 digest literals |
| [`ssrf`](src/ssrf.rs) | Shared static HTTPS/host/IP preflight policy and URL redaction |
| [`mcp`](src/mcp.rs), [`trace`](src/trace.rs) | Private MCP identifiers, optional-catalog error prefixes, and validated trace/correlation metadata |
| [`agent`](src/agent.rs), [`task`](src/task.rs), [`dev_container`](src/dev_container.rs) | Revision, session, intent, lifecycle, quota, image, and host-capability contracts |

## Metadata and wire compatibility

`ActionSpec` supplies action identity, parameters, descriptive return hints,
an optional complete output-schema function, and independent `requires_admin`
and `destructive` flags. Admin-only access and irreversible loss are different
policy axes; mutation alone does not imply destructive behavior. `ParamSpec.ty`
is a static string label, while `schema_for` generates a schema from a typed
payload. Product adapters own authorization, confirmation, and wire projection.

`PluginMeta` and `EnvVar` describe services and their configuration fields;
`UiSchema` describes field kind, validation, dependencies, and wizard hints.
Declaring an environment-variable name does not read it or register a service.
`file_path_within_root` is lexical UI validation: it rejects traversal components
but does not canonicalize paths or protect against symlinks. Actual I/O needs
the owning runtime's filesystem checks.

Public variants, fields, wire labels, schema versions, URI/scope compatibility
names, cancellation identifiers, and error-prefix strings are contracts. Keep
names such as `lab://`, `lab:read`, `lab`, and `lab:admin` across the product
rename. SDK compatibility re-exports in `labby-apis::core` refer to these same
types; change the canonical definitions here rather than creating another copy.

## Canonical JSON and digests

`to_canonical_vec` and `canonical_value` produce compact UTF-8 JSON with object
keys sorted by their UTF-8 bytes and deterministic string escaping. Numeric
values must be integers; floats fail with `CanonicalJsonError` rather than
acquiring an implementation-dependent signing or fingerprint representation.

`fingerprint_hex` returns lowercase SHA-256 hex without an algorithm prefix.
`canonical_json::digest` returns `Sha256Digest`, whose canonical wire form is
`sha256:` followed by lowercase hex. Use the appropriate form for the owning
contract; they are not interchangeable strings. Hashing establishes content
identity, not the sender's authority or a signature's validity.

This example follows the [canonical JSON tests](src/canonical_json.rs):

```rust
use labby_primitives::canonical_json::{fingerprint_hex, CanonicalJsonError};
use serde_json::json;

fn stable_fingerprint() -> Result<(), CanonicalJsonError> {
    let first = json!({"z": 1, "a": true});
    let reordered = json!({"a": true, "z": 1});
    assert_eq!(fingerprint_hex(&first)?, fingerprint_hex(&reordered)?);
    assert!(fingerprint_hex(&json!({"value": 1.5})).is_err());
    Ok(())
}
```

README code fences are explanatory; they are not included in Rustdoc doctests.

## Security and authority boundaries

`parse_validated_https_url` rejects forbidden URL components, private hosts,
and blocked IP literals using the shared SSRF policy. It performs **no DNS**.
Outbound runtimes must also validate resolved addresses, control redirects,
bind the connection to approved addresses, and check the connected peer where
the transport exposes it. Static preflight alone does not prevent rebinding.
Keep host/IP rules in this module rather than copying range checks into clients.

`access` validates identifiers and distinguishes durable ownership from
publication visibility. Its versioned capability and role-template vocabulary
rejects unknown wire values or unsupported schemas; consumers must treat that
result as denial. These types do not authenticate a caller or resolve current
membership/policy state. See [Access](../../docs/services/ACCESS.md) and the
[multi-user authority contract](../../docs/access-control/MULTI_USER_AUTHORITY.md).

`AuthorityProjectionEnvelope::within_bounds` checks structural identity,
sequence, digest shape, record count, and encoded size. It does not verify the
signature, prove the payload digest, apply records, or acknowledge durable
acceptance. Producers/consumers above this layer own signing, verification,
outboxes, replay protection, persistence, and freshness. Source constants own
the shared projection limits.

`ProductCredential` strictly parses the product-token wire form and deliberately
omits `Debug`, `Display`, serialization, and casual cloning. Its prefix selector
distinguishes malformed product credentials from non-product tokens; callers
must reject malformed selections rather than fall back to another verifier.
`ProductCredentialVerifier`, its non-secret grant,
and `BoundAccessGrant` are seams/data contracts; verification implementations
and current authorization decisions live above this crate.

Trace types validate and carry traceparent, tracestate, baggage, and Code Mode
correlation with source-owned bounds. Hosts decide whether to continue inbound
context and where to inject it. Baggage is observability data and must never
become tenant, routing, capability, or authentication authority. See the
[observability guide](../../docs/dev/OBSERVABILITY.md).

## Lifecycle data and state ownership

Agent definitions pin revisions, digests, capabilities, epochs, and sessions.
Task contracts describe intents, owner/project consistency, transitions,
attempt leases, and settlement; these are Agent Tasks rather than Depot jobs.
Dev Container contracts validate immutable image digests, quotas, explicit
host-capability policy, lifecycle identity, and secret references. Host access
is denied by default; constructing a value does not launch a resource or enforce
a daemon's runtime policy.

Callers supply configuration and current state, then persist or execute through
their owning layers. Random trace IDs and lifecycle nonces use operating-system
entropy and return typed errors when generation fails. Lexical path helpers use
the target platform's path semantics. No running gateway, upstream service, or
container backend is needed for these data/validation contracts.

## Verification

Use the workspace [Rust pin](../../rust-toolchain.toml),
[testing guide](../../docs/dev/TESTING.md), and [crate instructions](AGENTS.md).
From the repository root:

```bash
cargo nextest run -p labby-primitives
cargo clippy -p labby-primitives --all-targets -- -D warnings
```

Tests live beside their contracts and cover wire rejection, canonical encoding,
SSRF edge cases, credential selection, authority bounds, and lifecycle invariants.
Nextest excludes doctests; follow the root `just rustdoc-check` contract when
changing Rustdoc or its examples. Passing validation tests does not qualify a
consumer's authentication, persistence, networking, or resource execution path.
