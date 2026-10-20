# Labby Tailcat transport

Experimental native lifecycle supervisor for Labby's pinned Tailcat Go helper.
The crate validates helper bytes, owns their executable snapshot, drives the
control protocol, and terminates the helper's process group. It reuses
`labby-gateway`'s process guard. It does not authenticate users, approve pairings,
mint credentials, mount product routes, or build the Go helper automatically.

The `labby` product owns pairing, native session policy, credential custody, and
the restricted MCP projection. The Go helper in
[tools/tailcat-bridge](../../tools/tailcat-bridge) owns relay forwarding and
delivery encryption. Its browser WASM and the portable
[browser client](../../packages/labby-tailcat-browser) form the other transport
endpoint. Follow [architecture](../../docs/ARCH.md#crateslabby-tailcat) and the
[Tailcat browser guide](../../docs/guides/TAILCAT_BROWSER.md) for their wiring.

## Platform and prerequisites

`ensure_platform_supported()` explicitly accepts Linux and macOS. Lifecycle
types are compiled behind `cfg(unix)`; that compilation gate does not promise
support for every Unix target. Other platforms return `UnsupportedPlatform`.
The crate is internal (`publish = false`) and has no optional crate features.
The product selects it through its `tailcat` feature and separately enabled
operator configuration.

A Rust caller needs a Tokio runtime with process, I/O, and timer support, a
matching native helper, an existing absolute private state directory, one
approved browser public key, and an approved HTTPS DERP map. The helper needs
network access to that map/relay and the configured local target. Build Rust
with the repository's [toolchain](../../rust-toolchain.toml); build helper/WASM
with the Go version and dependency pins in its
[module](../../tools/tailcat-bridge/go.mod) and lockfile.

The target must be a separately authorized restricted MCP listener. A numeric
loopback address limits routing but does not authorize requests. Native
per-request authority, revocation, installation binding, and tool restrictions
belong to the product integration; do not target a general operator API.

## Configuration and verified executable custody

The public [BridgeConfig](src/config.rs) contains these explicit inputs:

| Field | Contract |
| --- | --- |
| `executable` | Absolute path to a regular, non-symlink helper file, at most 128 MiB. |
| `state_dir` | Absolute existing directory where startup creates its private snapshot. The caller owns directory privacy and access policy. |
| `expected_sha256` | Independently trusted 32-byte digest of the native helper. |
| `target` | `SocketAddr` with a numeric loopback IP and nonzero port, including IPv4-mapped loopback handling. |
| `peer` | One approved browser key: `nodekey:` followed by 64 lowercase hexadecimal digits, excluding the all-zero key. |
| `derp_map_url` | HTTPS URL with a host, no user information, and no fragment; the caller owns relay approval. |

`BridgeConfig::validate()` checks these values synchronously, reads bounded
helper bytes, verifies SHA-256, and returns opaque `ValidatedBridgeConfig`.
Unix file opening uses `O_NOFOLLOW`. Startup writes the captured bytes to a
private temporary executable with mode `0500`, then launches that snapshot.
Replacing the original file after validation cannot change those captured
bytes. The snapshot remains owned by the `Bridge` until it is dropped.

A checksum bundled with an untrusted download does not independently establish
trust. Pin or verify the native digest through the product's trusted asset
policy; browser WASM and its Go runtime need independently trusted hashes too.

## Public lifecycle API

| API | Behavior |
| --- | --- |
| `Bridge::start(validated)` | Starts one helper and waits up to 30 seconds for protocol-v1 readiness. |
| `Bridge::start_with_timeout(validated, timeout)` | Accepts a nonzero startup bound of at most 30 seconds, including snapshot preparation. |
| `capability()` | Returns `ConnectionCapability`, with explicit `address()` and `port()` accessors for authorized delivery. |
| `status(&mut self)` | Returns `BridgeStatus::{Ready, Stopped, Failed}` and observes helper failure without disclosing its capability. |
| `seal_delivery(&mut self, payload)` | Encrypts one nonempty JSON delivery of at most 16 KiB to the fixed approved peer; waits up to 10 seconds. |
| `stop(&mut self)` | Performs async shutdown; repeat calls are supported. Clears the capability address and sets `Stopped`. |

This integration shape assumes the product has already authorized and assembled
`config`. The caller retains the returned bridge for the paired session:

```rust
use labby_tailcat::{Bridge, BridgeConfig, BridgeError};

async fn start_transport(config: BridgeConfig) -> Result<Bridge, BridgeError> {
    labby_tailcat::ensure_platform_supported()?;
    Bridge::start(config.validate()?).await
}
```

Use `seal_delivery` once when the product is ready to publish its authorized
delivery; retain `SealedDelivery.sender` and `.ciphertext` only for that flow.
Keep the bridge alive while the browser uses the session, then await `stop()`.
This crate has no automatic restart or retry of sealing after an uncertain
result. A new helper requires a fresh product-owned lifecycle decision.

## Protocol, shutdown, and secrets

[protocol.rs](src/protocol.rs) defines newline-delimited JSON control frames
with `version: 1` and `type`, capped at 64 KiB. Startup sends the fixed target,
peer, and map to the helper over stdin. Readiness requires one valid address
and virtual port `1`. A solicited `sealed` event can complete the single seal;
malformed, extra, or unsolicited events retire the helper without waiting for
a status poll. Helper stdout is private protocol data; stderr is drained to
a sink rather than copied into product logs.

The Go helper admits the configured peer and caps forwarding at eight concurrent
connections. Local dials have a five-second timeout; forwarded reads and writes
refresh 30-second idle deadlines. These transport limits do not grant MCP access.

The helper runs in its own process group with a cleared environment and
kill-on-drop enabled. `stop()` allows up to two seconds for its stop command,
stdin closure, and child exit, then drops the group guard. If graceful shutdown
fails, it requests a kill and waits up to two more seconds. These waits are
bounded; the final forced-wait result is not surfaced as a separate receipt.

Cancellation during startup, sealing, or stop releases the owned kill guard.
Dropping `Bridge` also terminates owned descendants and retires its snapshot;
normal async stop additionally waits for the child. Snapshot creation runs on
a blocking worker: cancellation cannot interrupt an executing filesystem call,
but an unclaimed result drops its temporary files. No helper starts before
snapshot preparation completes.

`BridgeConfig`, `ConnectionCapability`, and `SealedDelivery` redact Debug
output. `BridgeError` uses stable categories without paths, keys, or raw helper
output. Explicit address/port and delivery-field access still returns sensitive
data; publish it only through the authorized pairing response. Ordinary
`BridgeStatus` contains no connection address or private key.

## Build and focused verification

From the repository root, with the pinned Rust/Go environments and Node for
WASM tests:

```sh
bash scripts/build-tailcat-bridge.sh /absolute/owned/build-directory
bash scripts/test-tailcat-bridge.sh
cargo nextest run -p labby-tailcat
cargo clippy -p labby-tailcat --all-targets -- -D warnings
```

The build script verifies Go modules and emits the native helper, browser WASM,
matching `wasm_exec.js`, compressed WASM, notices, and `SHA256SUMS`. It needs
`gzip` and either `sha256sum` or `shasum`. The test script runs Go race tests and
Node-hosted Go WASM tests; its temporary wrapper honors `TMPDIR`.

Rust [config tests](tests/config.rs), [lifecycle tests](tests/lifecycle.rs), and
snapshot tests cover validation, immutable captured bytes, redaction, malformed
protocol, bounded startup, single-use sealing, cancellation, repeated/forced
stop, and descendant cleanup using shell helpers. Go tests use a disposable
DERP/TLS fixture to check wrong-peer rejection, approved forwarding, and stream
closure. WASM tests check callback release and encrypted-delivery bindings.

The [browser fixture](../../tools/tailcat-bridge/integration/browser-fixture.mjs)
uses actual WASM and the native helper with synthetic MCP through a public test
relay; eight overlapping requests share one client and relay identity. It
does not qualify product authorization, VM execution, or a deployed Depot page.
The separate [native browser acceptance fixture](../../tools/tailcat-bridge/integration/native-vm-browser.mjs)
requires explicit owned paths, native authority, a browser, and a real VM
runtime. Follow the [browser guide](../../docs/guides/TAILCAT_BROWSER.md) for
product setup and its qualification boundaries; fixture results do not establish
that an external deployment or relay configuration is ready.
