---
title: "Browser Bridge"
created: "2026-08-29"
updated: "2026-09-05"
---

# Browser Bridge

The `browser` service is Labby's Rust-native bridge to browser WebMCP tools. Labby owns durable browser identities, operator-approved pairing, authenticated live connections, sanitized catalog persistence, explicit per-document enablement, bounded invocation routing, cancellation, and stale-document protection. It does not run a separate Webby, Phoenix, LiveView, or Next.js application.

An unpacked Manifest V3 extension lives in `apps/browser-extension`. JavaScript remains only at the browser boundary because Chrome executes the WebMCP probe and tool call inside the page's main world. The extension connects to Labby's `/browser/socket` WebSocket on loopback and speaks the versioned JSON protocol implemented by `labby-browser`.

## Trust and consent

Pairing uses an extension-generated Ed25519 identity and requires operator approval through the `browser` service. The private key is non-extractable and is stored as a structured-cloned `CryptoKey` in the extension's IndexedDB database; it is never exported to or stored in `chrome.storage.local`. Authentication challenges are short-lived and single-use. The WebSocket adapter accepts only loopback peers with a browser-extension origin.

The extension intentionally fails closed when identity state is missing, corrupt, revoked, or still in the legacy extractable-JWK format. It deletes the unusable credential and its `browserId`/pending pairing association, generates a fresh non-extractable identity, and requires the operator to pair and approve it again. Removing and reinstalling the extension likewise loses the device credential and requires re-pairing. There is no key export or recovery phrase; recovery is revocation followed by a new operator-approved pairing.

Discovery stores only the origin, sanitized path, title, immutable Chrome document identity, catalog revision/fingerprint, and bounded tool metadata. It does not store page contents, cookies, form values, or executable callbacks. Observed sessions begin disabled; an administrator must explicitly enable the exact session before `browser.call` can invoke a tool. Calls fail closed when the browser disconnects, the document or catalog revision changes, the tool is absent, capacity is exhausted, or the deadline expires.

`browser.sessions` returns metadata-only pages of at most 100 sessions and an opaque `next_cursor`; it never expands stored tool schemas. Use `browser.session.get` with an exact `session_id` when an operator needs the bounded catalog detail for one document. Browser SQLite work runs through a bounded blocking executor so database contention does not occupy Tokio request workers. The store applies numbered migrations transactionally and refuses databases created by a newer Labby version.

Use the generated [service catalog](../generated/service-catalog.md) and [action catalog](../generated/action-catalog.md) for the exact registered surfaces, parameters, and scope requirements.

## Extension checks

```bash
npm ci --prefix apps/browser-extension
npm test --prefix apps/browser-extension
npm run typecheck --prefix apps/browser-extension
```
