# labby-browser

`labby-browser` is Labby's surface-neutral Browser WebMCP runtime. It owns
SQLite-backed pairing and document catalogs, Ed25519 challenge authentication,
live connection generations, invocation routing/cancellation, and metadata-only
audits. It is an internal library (`publish = false`), not a WebSocket server
or a browser automation client.

The [product browser contract](../../docs/services/BROWSER.md) describes operator
behavior. [The HTTP adapter](../labby/src/api/services/browser.rs),
[shared product dispatch](../labby/src/dispatch/browser.rs), and
[Chrome extension](../../apps/chrome) supply transport admission, product
permissions, browser site access, and page execution around this library.

## Public API and architecture

[src/lib.rs](src/lib.rs) re-exports the supported library surface:

| Interface | Responsibility |
| --- | --- |
| `BrowserBridge` | Cloneable runtime facade over durable state and the live hub; pairing, authentication, observations, invocation, and revocation. |
| `BrowserConnection`, `BrowserEvent` | Adapter-owned connection identity and bounded receiver of outbound envelopes. Dropping the connection disconnects only its generation. |
| `BrowserEnvelope`, `BrowserMessage` | Typed versioned protocol, request correlation, pairing/authentication, catalog events, calls, cancellations, and replies. |
| `CatalogObservation`, `ToolDescriptor` | Document-bound metadata and JSON schemas; executable callbacks never cross this wire. |
| `Store` | Cloneable asynchronous facade over serialized SQLite work on Tokio's blocking pool. |
| `BrowserRecord`, `PairingRequest`, `PairingStatus` | Durable browser identity and operator approval state. Public-key fields are excluded from normal serialization; private keys never enter the runtime. |
| `DocumentSession`, `DocumentSessionSummary`, `SessionPage` | Exact catalog detail, metadata-only listings, and bounded cursor pagination. |
| `BrowserStorageLock` | Offline ownership of the same protected storage directory used by the live runtime, without opening SQLite. |
| `BrowserError`, `Result` | Typed failures retained until the product surface maps them. |

The private [hub](src/hub.rs), [protocol](src/protocol.rs), and
[store](src/store.rs) modules implement these exports. [Cargo.toml](Cargo.toml)
declares no crate features and no Axum, Clap, or rmcp dependency. It uses
`labby-auth` with empty defaults for shared authentication helpers; Unix storage
uses rustix, and Windows storage uses `labby-winjob`.

## Initialize and retain a runtime

`BrowserBridge::open` accepts an explicit database path, protects/locks storage,
applies migrations, and creates a live hub. Run it inside a Tokio runtime and
retain the returned bridge for the owning service. Clones share its store and
hub. `BrowserBridge::memory` and `Store::memory` provide isolated in-memory
state for tests without opening protected filesystem storage.

This helper opens a runtime and returns its first metadata-only session page:

```rust
use labby_browser::{BrowserBridge, Result, SessionPage};
use std::path::PathBuf;

async fn open_bridge(database: PathBuf) -> Result<(BrowserBridge, SessionPage)> {
    let bridge = BrowserBridge::open(database).await?;
    let page = bridge.store().sessions(None, Some(25)).await?;
    Ok((bridge, page))
}
```

Use an absolute, traversal-free database path beneath an owned private directory.
Opening state does not bind a listener, install an extension, approve a browser,
or authorize a page tool. The product supplies its selected installation path;
this crate does not discover an ambient product configuration.

## Pairing, authentication, and catalog consent

1. A validated extension adapter submits `request_pairing` with its public key,
   package ID, and display name. The short-lived request exposes an out-of-band
   fingerprint that binds approval to that exact request and key.
2. An authorized operator confirms the fingerprint through
   `BrowserBridge::approve_pairing`. The public key identifies a browser
   installation/profile; a package ID alone does not prove identity. Distinct
   keys under the same extension ID can remain paired concurrently.
3. `issue_challenge` returns a short-lived, single-use challenge value (nonce).
   The extension signs its raw
   bytes with Ed25519; `authenticate` validates the signature and atomically
   consumes the challenge before installing a connection generation. A bad
   signature does not consume a valid challenge.
4. The adapter forwards observations with that connection's browser and
   connection IDs. Sessions begin disabled. An operator reviews exact session
   detail and submits its server-computed `catalog_digest` when enabling it.
5. Calls name the browser, tab, immutable document ID, catalog revision/digest,
   and an observed tool. The server digest binds the actual catalog contents,
   origin, revision, and fingerprint. Reusing a peer fingerprint cannot preserve
   consent after changing a schema or document.

Use the bridge's approval/revocation methods when a live hub exists: they
coordinate durable identity with current connections and pending calls.
`revoke_browser` closes/disables durable sessions and ends live work. Re-pairing
a credential evicts its superseded identity. A stale socket cannot disconnect
its replacement or complete calls admitted under another generation.

The adapter must authenticate administrative actions and validate socket
Origin/Host policy. The extension must enforce site permissions and exact page
execution bindings. Catalog descriptions and annotations remain untrusted;
operator consent is separate from product destructive-action authorization.
Follow [the browser contract](../../docs/services/BROWSER.md) for those policies.

## Protocol and invocation lifecycle

`PROTOCOL_VERSION` is `2`; `LEGACY_PROTOCOL_VERSION` is `1`.
`BrowserEnvelope::new` emits v2 and `validate_version` accepts v2 only.
`validate_server_version` accepts v1/v2 for runtime compatibility and rejects
unknown versions. Adapters preserve the negotiated version with `for_version`;
a compatibility check does not establish the behavior of an installed extension.

The hub admits at most 100 pending calls and uses a bounded event channel per
connection. It validates the exact enabled catalog before admission and again
before dispatch after audit I/O. Observation changes, document closure,
disconnect, and replacement invalidate the affected pending calls. Late or
wrong-generation completions return no accepted result.

`call` defaults its reply-wait timeout to 15 seconds and accepts an explicit
`Duration`. Timeout emits `ToolCancel`; dropping an in-flight call also cancels
its exact pending entry and schedules bounded audit cleanup. This reply timer
is not an end-to-end transport or SQLite deadline: the adapter owns its socket,
request, and shutdown limits. Cancellation cannot prove a page callback rolled
back effects that already occurred.

Invocation audits retain tool/session identities, catalog revision, outcome,
error kind, duration, and timestamps. They omit arguments and result bodies.
Cleanup preserves selected terminal outcomes and bounds its backlog. Restart
migration marks unfinished audits abandoned rather than reporting success.

## Storage and errors

The [storage implementation](src/store/storage.rs) requires one process owner
through a persistent lock file, retained until SQLite closes. It rejects path
aliases and protects the database plus WAL/SHM/journal files. Unix validates
ownership, private modes, ancestors, and hard links; Windows uses native DACL,
reparse-point, link, and directory-handle checks. Existing shared directories
fail visibly instead of receiving a silent permission repair. Offline backup
and restore use `BrowserStorageLock::acquire_existing` against the same path.

Numbered migrations use a transactional `browser_meta` schema marker and reject
newer databases. Credential-identity migration revokes ambiguous duplicated
active keys and pending approvals rather than choosing an identity. Pairings,
challenges, catalogs, and session pages have bounded admission/size rules;
listings return summaries, while exact session lookup returns catalog detail.
Adapters supply sanitized URL/title metadata, never page contents, cookies,
form values, or callbacks for persistence.

`BrowserError::kind()` distinguishes `auth_failed`, `browser_offline`,
`server_busy`, `tool_timeout`, `cancelled`, and `stale_document`, plus validation
and internal failures. Storage/JSON failures remain internal errors rather than
appearing as a disconnected browser. Preserve these distinctions and avoid
retrying a page mutation merely because its result was lost.

## Verification

Run from the workspace root with the pinned toolchain:

```bash
cargo check -p labby-browser --all-targets
cargo nextest run -p labby-browser
cargo nextest run -p labby-browser --lib hub::tests
cargo nextest run -p labby-browser --lib store::tests
cargo nextest run -p labby-browser --lib store::storage_tests
cargo nextest run -p labby-browser --lib protocol::tests
cargo clippy -p labby-browser --all-targets -- -D warnings
```

There are no feature slices. Library tests cover cryptographic/protocol fixtures,
storage migrations/protection, consent, cancellation, and races. They do not
qualify a real Chrome/WebMCP page. Extension checks belong to `apps/chrome`;
product fixtures and real-browser evidence follow [Testing](../../docs/dev/TESTING.md)
and [the browser contract](../../docs/services/BROWSER.md). Nextest excludes
doctests; `just rustdoc-check` covers workspace Rustdoc and doctests. This README's
Rust fence is illustrative and is not an executable doctest.
