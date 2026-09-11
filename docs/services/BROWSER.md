---
title: "Browser Bridge"
created: "2026-08-29"
updated: "2026-09-10"
---

# Browser Bridge

The `browser` service is Labby's Rust-native bridge to browser WebMCP tools. Labby owns durable browser identities, operator-approved pairing, authenticated live connections, sanitized catalog persistence, explicit per-document enablement, bounded invocation routing, cancellation, and stale-document protection. It does not run a separate Webby, Phoenix, LiveView, or Next.js application.

An unpacked Manifest V3 extension lives in `apps/browser-extension`. JavaScript remains only at the browser boundary because Chrome executes the WebMCP probe and tool call inside the page's main world. The extension connects to Labby's `/browser/socket` WebSocket and speaks the versioned JSON protocol implemented by `labby-browser`. Loopback endpoints may use HTTP/WS; remote endpoints must use HTTPS/WSS and still pass configured Host validation. Chrome 116 or newer is required; the extension sends an acknowledged versioned heartbeat every 20 seconds so WebSocket activity keeps the Manifest V3 service worker alive during otherwise idle periods and a half-open path is forced through normal reconnect handling when the heartbeat reply times out.

## Trust and consent

`browser.call` is classified as destructive because page callbacks may delete durable data. Page-provided read-only annotations cannot override this classification. Administrator access and catalog consent remain separate requirements; MCP clients use the gateway’s existing destructive-action confirmation contract.

Pairing uses an extension-generated Ed25519 identity and requires operator approval through the `browser` service. The Chrome extension id identifies the extension package for Origin admission; the Ed25519 public key identifies one browser installation/profile, so multiple installations of the same extension may remain paired concurrently. The private key is non-extractable and is stored as a structured-cloned `CryptoKey` in the extension's IndexedDB database; it is never exported to or stored in `chrome.storage.local`. Authentication challenges are short-lived, single-use, globally bounded, and limited to one outstanding challenge per browser identity; consumed, expired, or superseded challenge rows are pruned when Labby issues a new challenge. A pending pairing retry may be reused only by the same extension public key and never extends the request’s original expiry; a conflicting key cannot mutate an operator-visible approval target. Each pending request also has a short fingerprint derived from the request identity and public key. The extension displays that fingerprint, and `browser.pairing.approve` requires the operator to submit the matching value before Labby creates a durable browser identity. The WebSocket Origin syntax check and configured Host validation are admission filters, not proof of browser identity; a remote browser becomes trusted only after fingerprint-confirmed pairing and subsequent Ed25519 challenge authentication.

The extension intentionally fails closed when identity state is missing, corrupt, revoked, or still in the legacy extractable-JWK format. It deletes the unusable credential and its `browserId`/pending pairing association, generates a fresh non-extractable identity, and requires the operator to pair and approve it again. Removing and reinstalling the extension likewise loses the device credential and requires re-pairing. There is no key export or recovery phrase; recovery is revocation followed by a new operator-approved pairing.

The validated extension socket initializes the local daemon's browser runtime. Until that happens, runtime actions report `browser_unavailable`; `help` and `schema` remain available without opening browser state. CLI or stdio processes must target the owning daemon rather than creating a disconnected second browser runtime.

Browser storage admits one process owner through a persistent lock and requires an owned private directory with unaliased SQLite files and sidecars. Offline backup and restore share that ownership lock. Existing shared directories are refused without silently changing their permissions; repair the directory explicitly before restarting. The socket adapter admits at most 64 connections overall, at most 8 simultaneous unauthenticated handshakes per client address, and at most 8 successful pairing requests per client address in a five-minute admission window; every unauthenticated handshake is bounded to two minutes and writes are bounded to five seconds. The per-client permit is released immediately after Ed25519 authentication, so established sockets do not consume pre-authentication allowance. Direct socket peers define the client address by default; when `api.trust_forwarded_headers` is enabled behind a sanitizing reverse proxy, the final `X-Forwarded-For` address defines that admission bucket.

Once initialized, the runtime remains available after the extension socket disconnects. Initialization failures are cached for the lifetime of the daemon; correct the underlying problem and restart the daemon before retrying.

Extension authentication and page-call replies are bound to the socket generation that started them. Disconnects cancel that generation's pending page calls; late results cannot be sent through a replacement connection. Each page retains at most 1,024 cancellation/call records. If cancellation evidence exceeds this bound, new invocations fail closed until the page is reloaded, rather than evicting evidence that could allow a delayed invocation to run.

Discovery stores only the origin, sanitized path, title, immutable Chrome document identity, catalog revision/fingerprint, and bounded tool metadata. It does not store page contents, cookies, form values, or executable callbacks. Observed sessions begin disabled. An administrator reads `browser.session.get` and submits its server-computed `catalog_digest` with both `browser.session.enable` and `browser.call`. The digest binds the reviewed origin, revision, fingerprint, and actual tool schemas. Reusing a peer-supplied revision or fingerprint cannot preserve consent after the catalog changes. Calls fail closed when the browser disconnects, the document or catalog revision changes, the tool is absent, capacity is exhausted, or the deadline expires.

`browser.sessions` returns metadata-only pages of at most 100 sessions and an opaque `next_cursor`; it never expands stored tool schemas. Use `browser.session.get` with an exact `session_id` when an operator needs the bounded catalog detail for one document. Browser SQLite work runs through a bounded blocking executor so database contention does not occupy Tokio request workers. The store applies numbered migrations transactionally and refuses databases created by a newer Labby version. The v3-to-v4 credential migration fails closed on ambiguous reused public keys by revoking all duplicated active identities, closing their sessions, invalidating their challenges, and expiring duplicated pending approvals before installing the new credential-uniqueness indexes.

Use the generated [service catalog](../generated/service-catalog.md) and [action catalog](../generated/action-catalog.md) for the exact registered surfaces, parameters, and scope requirements.

## Extension checks

```bash
npm ci --prefix apps/browser-extension
npm test --prefix apps/browser-extension
npm run typecheck --prefix apps/browser-extension
```
