# gateway-admin — Operator Web UI

This Next.js/React application is Labby's operator web UI. It talks to the Rust backend over supported HTTP endpoints and is exported as static assets for embedding/serving by Labby.

## UI Rules

- reuse Aurora design tokens, components, spacing, typography, and page patterns
- reuse existing components before inventing parallel primitives
- keep browser data flows aligned with backend redaction and auth contracts
- do not embed administrator bearer secrets in browser bundles
- same-origin hosted mode uses Rust-owned browser session/CSRF behavior
- keep mock data explicitly development-only

## Engineering Rules

The app's module/test contract is documented in README.md and guarded by tests. Use existing npm/pnpm scripts rather than inventing a second toolchain. Browser-only behavior needs browser tests; pure data/component logic should stay testable in the normal unit path.

Read `components/ui/CLAUDE.md` before modifying shadcn primitives. Product design contracts live in `docs/design/`.
