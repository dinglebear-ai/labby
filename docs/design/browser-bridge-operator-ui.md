---
title: "Browser Bridge Operator UI"
created: "2026-08-29"
updated: "2026-08-29"
---

# Browser Bridge Operator UI

## Problem

The Rust browser bridge needs an operator-facing lifecycle in Labby's existing Next.js console. CLI, MCP, and raw API actions are insufficient for reviewing extension identities, granting execution consent, and understanding which live page catalog will receive a call.

## Structure and data

The `/browsers` route is a Control Plane destination. It uses only the shared authenticated `/v1/browser` service-action adapter and renders server truth:

- a compact health strip for paired, pending, observed, and enabled counts;
- pending pairing cards with extension identity and expiry;
- paired browser cards with live/offline/revoked state and revocation;
- active document cards with sanitized page identity, observed tools, catalog revision, and an explicit execution switch.

The page polls every five seconds because browser connections and page navigations change independently of operator interaction. Mutations refresh all three datasets after success.

## Interaction and safety

Pairing approval is explicit. Session execution begins disabled and uses the exact backend session identity. Revocation requires confirmation and tells the operator that reconnecting requires a new pairing. Backend authorization, tuple validation, catalog membership, and stale-document rejection remain authoritative.

The UI never accepts arbitrary tool arguments or executes page tools directly. It does not display persisted page content because the bridge stores only sanitized identity and catalog metadata.

## States and accessibility

Loading, empty, populated, mutation-busy, and backend-error states are represented. Switches have page-specific accessible labels, headings label each region, and all mutations use standard Aurora focusable controls. Cards collapse to one column on narrow screens, while long extension IDs and URLs truncate or wrap without widening the viewport.

No new design token or primitive is introduced. The page reuses the console hero, Aurora cards, alerts, badges, buttons, switches, empty states, and confirmation dialog.
