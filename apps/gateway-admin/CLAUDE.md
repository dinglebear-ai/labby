# Labby Gateway Admin Instructions

This directory is the Next.js operator UI embedded by Labby. The app currently uses Next.js 16, React 19, Tailwind CSS 4, pnpm 9, and a static export consumed by the Rust host.

## Boundaries

- Keep backend semantics in Labby dispatch/runtime code. The UI should call typed API helpers and render server truth rather than reimplementing action policy.
- Reuse `lib/api/`, `lib/http/`, `lib/auth/`, and existing hooks before adding transport wrappers.
- Product pages live under `app/(admin)/`; reusable feature components live in `components/<feature>/`.
- The embedded export must keep working without a standalone Node server.
- Do not add direct arbitrary host filesystem, shell, or MCP access from browser code.

## Design System

Aurora is the source of truth for styling. Read `components/ui/CLAUDE.md`, `docs/design/design-system-contract.md`, and `docs/design/component-development.md` before UI work. Reuse existing tokens and primitives; do not introduce duplicate buttons, dialogs, inputs, tables, badges, spacing scales, or raw theme colors.

## Safety And Errors

Mutations must preserve backend authorization, destructive-action, CSRF/session, and redaction contracts. Surface structured backend errors with actionable recovery instead of replacing them with generic strings. Never log authorization headers, OAuth material, setup secrets, or raw secret-valued form fields.

## Verification

Run the checks that match the change:

```bash
cd apps/gateway-admin
pnpm lint
pnpm test
pnpm test:browser
pnpm build
```

A production UI change is not complete until the static export succeeds and the relevant route/component tests pass.
