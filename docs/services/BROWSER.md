---
title: "Browser Bridge"
created: "2026-08-29"
updated: "2026-08-29"
---

# Browser Bridge

The `browser` service is Labby's Rust-native bridge to browser WebMCP tools. Labby owns durable browser identities, operator-approved pairing, authenticated live connections, sanitized catalog persistence, explicit per-document enablement, bounded invocation routing, cancellation, and stale-document protection. It does not run a separate Webby, Phoenix, LiveView, or Next.js application.

An unpacked Manifest V3 extension lives in `apps/browser-extension`. JavaScript remains only at the browser boundary because Chrome executes the WebMCP probe and tool call inside the page's main world. The extension connects to Labby's `/browser/socket` WebSocket on loopback and speaks the versioned JSON protocol implemented by `labby-browser`.

## Trust and consent

Pairing uses an extension-generated Ed25519 identity and requires operator approval through the `browser` service. Authentication challenges are short-lived and single-use. The WebSocket adapter accepts only loopback peers with a browser-extension origin.

Discovery stores only the origin, sanitized path, title, immutable Chrome document identity, catalog revision/fingerprint, and bounded tool metadata. It does not store page contents, cookies, form values, or executable callbacks. Observed sessions begin disabled; an administrator must explicitly enable the exact session before `browser.call` can invoke a tool. Calls fail closed when the browser disconnects, the document or catalog revision changes, the tool is absent, capacity is exhausted, or the deadline expires.

Use the generated [service catalog](../generated/service-catalog.md) and [action catalog](../generated/action-catalog.md) for the exact registered surfaces, parameters, and scope requirements.

## Extension checks

```bash
npm ci --prefix apps/browser-extension
npm test --prefix apps/browser-extension
npm run typecheck --prefix apps/browser-extension
```
