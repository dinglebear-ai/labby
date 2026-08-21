---
title: "Component Development Process"
created: "2026-07-30"
updated: "2026-08-18"
---

# Component Development Process

**Status:** Active  
**Scope:** `apps/gateway-admin` web UI feature and component work  
**Primary contract:** [Labby Design System Contract](./design-system-contract.md)

## Purpose

Use this workflow for substantial Labby web UI features, major component rewrites, and design explorations. The goal is to review layout and interaction early, then land production React components that reuse Aurora primitives and current backend contracts.

## Required Workflow

### 1. Inspect The Current Product

Before designing, inspect the relevant route, existing Aurora components, nearby feature patterns, and the backend data/actions that the page actually consumes. Do not design from an old mockup or historical implementation plan when current code is available.

### 2. Write A Focused Design Spec

For substantial changes, capture the user problem, page structure, interaction model, data sources, loading/empty/error states, responsive behavior, accessibility expectations, and any intentional design-system deviations. Keep the spec current as decisions change.

### 3. Reuse Aurora

Review [design-system-contract.md](./design-system-contract.md) and the nested `apps/gateway-admin/CLAUDE.md` instructions. Reuse existing tokens, components, density, spacing, typography, surfaces, focus behavior, and motion patterns before creating a new primitive.

### 4. Use A Mockup When Visual Direction Is Unsettled

Labby has a development-only HTML mockup viewer for rapid visual iteration. Place a self-contained HTML file under:

```text
~/.superpowers/brainstorm/content/
```

The backend serves the newest HTML file at:

- `/dev/mockup` for the newest mockup overall
- `/dev/mockup/<name>` for the newest mockup whose filename stem contains `<name>`

The implementation lives in `crates/labby/src/api/dev_mockup.rs`; routing is registered in `crates/labby/src/api/router.rs` before the static web-app fallback. Named fragments reject path separators and `..`. Filesystem discovery runs on a bounded blocking pool.

When credential auth is configured, these routes use the same auth layer as other protected operator routes. They are not a parallel API surface and they do not provide special mutation bypasses or read-only service endpoints.

Mockups are ephemeral design work products. Do not commit them to the repository. Once the direction is approved, implement the feature in the real web app and discard the mockup.

### 5. Build The Production React Surface

Production pages live under the existing Next.js route structure, normally `app/(admin)/...`. Reuse the typed API clients, hooks, session/auth machinery, and shared feature components already used by adjacent pages. Do not create a second transport or policy layer in browser code.

Backend authorization, destructive-action policy, validation, and redaction remain authoritative. The UI should surface structured failures and recovery guidance rather than weakening those contracts.

### 6. Review The Real Component

Before calling a UI change complete, inspect the actual production component rather than only the mockup. Check:

- dark and light appearance when relevant
- desktop and narrow/mobile layouts
- loading, empty, populated, error, and disabled states
- keyboard navigation and visible focus
- accessible labels and semantics
- console errors and failed network requests
- consistency with Aurora and adjacent Labby pages
- mutation behavior and backend error handling

### 7. Verify

Run the checks appropriate to the change:

```bash
cd apps/gateway-admin
pnpm lint
pnpm test
pnpm test:browser
pnpm build
```

For changes to the Rust mockup server or web embedding boundary, also run the focused Rust tests for the changed module or crate.

## Design-System Deviations

A deviation from the design-system contract should be explicit, justified by the feature's user-visible needs, and reviewed as part of the feature design. Do not accumulate one-off tokens or duplicate primitives simply because a mockup used them.
