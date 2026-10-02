# Labby Tailcat transport

Experimental native lifecycle crate for a pinned Tailcat Go helper. This crate
supervises transport; it does not authenticate users, approve pairings, mint
credentials, or expose a Labby service by itself.

## Build

From the Labby repository root, using the Go version in
`tools/tailcat-bridge/go.mod`:

```sh
bash scripts/build-tailcat-bridge.sh /absolute/owned/build-directory
cargo test -p labby-tailcat
cargo clippy -p labby-tailcat --all-targets -- -D warnings
```

The build produces the native helper, browser WASM, matching `wasm_exec.js`,
compressed WASM, notices and `SHA256SUMS`. A product caller supplies an independently
trusted native checksum; accepting a checksum from an untrusted download would
not establish trust. Browser assets likewise need independently trusted hashes.

## Native contract

1. An authorized product owner constructs `BridgeConfig` with an absolute
   executable and private state directory, one numeric loopback target, one
   allowed browser public key and an approved HTTPS DERP map.
2. `validate()` verifies the artifact and captures its bytes. Spawn uses a
   private executable snapshot so replacing the source cannot change execution.
3. `Bridge::start()` awaits a bounded protocol-v1 ready event. The secret
   connection address is explicitly accessible through `capability()` and is
   excluded from Debug and ordinary status.
4. `stop()` allows two seconds, then kills the owned process group and reaps the
   helper. Cancellation/drop terminate owned descendants. Invalid subsequent
   events kill and reap without requiring a status poll. No automatic restart.

The target must be a separately authorized restricted MCP listener. A loopback
address is a network boundary, not authorization. Do not target a general
operator API. Native per-request authority, revocation, installation binding and
tool restriction remain prerequisites for the product integration.

## Verification boundaries

- Rust tests cover immutable artifacts, protocol failures, startup cancellation,
  graceful/forced/repeated stop and descendant cleanup.
- Go tests use a disposable DERP fixture and TLS map. An unapproved peer reaches
  no backend; an approved peer completes a round trip; stop closes its stream.
- `integration/browser-fixture.mjs` tests the portable JS client and actual Go
  WASM against a synthetic MCP backend through the public test relay. It is
  explicitly a fixture, not a production pairing endpoint or VM qualification.
- Native pairing, product listener, Depot LiveView and Microsandbox acceptance
  are described in the approved implementation plan and are not wired yet.
