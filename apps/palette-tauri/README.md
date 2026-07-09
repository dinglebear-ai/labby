# Labby Palette Tauri

Tauri v2 desktop command palette for a `labby serve` instance. The renderer is
React with Aurora registry components; the Rust shell owns server URL
resolution, OAuth/static bearer auth, and all HTTP traffic.

The palette launches hidden, registers a global shortcut, and exposes a tray
menu for showing the palette, opening settings, and quitting. The main window is
an undecorated transient palette that hides on Escape, close, and blur by
default.

## Launcher Model

The first screen is a unified launcher over:

- Labby product actions from the backend launcher catalog.
- Connected upstream MCP tools discovered by the Labby gateway.

The renderer calls fixed Tauri commands only:

- `fetch_launcher_catalog` -> `GET /v1/palette/catalog`
- `execute_launcher_entry` -> `POST /v1/palette/execute`

Renderer code never calls MCP or arbitrary HTTP directly. The Rust bridge builds
fixed `/v1/palette/*` URLs from the saved Labby server URL and sends requests
through the shared `send_with_reauth` path. OAuth tokens and static bearer tokens
stay in the Rust shell.

The backend catalog is a display hint, not an authorization decision. Execution
re-resolves the live upstream tool, re-checks scope/destructive policy, validates
against the current server-side schema, and dispatches through the existing
gateway upstream pool.

## Commands

```bash
pnpm install --frozen-lockfile
pnpm test
pnpm typecheck
pnpm vite:build
pnpm verify
pnpm dev
pnpm vite:dev
pnpm build
cargo test --manifest-path apps/palette-tauri/src-tauri/Cargo.toml
```

`pnpm build` runs a full Tauri release build and bundles platform packages.
`pnpm vite:build` is the faster frontend-only production build.

## Configuration

The app reads Labby connection settings from environment defaults first:

- `LABBY_PUBLIC_URL`
- `LABBY_MCP_HTTP_TOKEN`

Runtime palette preferences are stored in the platform app config directory as
`settings.json`. The settings panel can override the server URL, static bearer
token, shortcut, theme, result layout, footer hints, and hide-on-blur behavior.

## Authentication

The palette authenticates to Labby two ways, and both can be configured:

- Static bearer token from `LABBY_MCP_HTTP_TOKEN` or the settings panel.
- OAuth "Sign in with Google" through the Rust shell's Authorization Code + PKCE
  flow.

When a valid OAuth credential exists for the active server, it takes precedence
over the static token. If OAuth is unavailable or expired, the bridge falls back
to the static token when configured.

## Schema Validation

The backend sends a redacted schema projection for launcher forms. The renderer
uses Ajv for best-effort JSON Schema validation before submit, memoized by
`entry.id + schemaFingerprint`. Unknown or unsupported schemas fail open in the
renderer; the backend remains the authoritative validator.

The schema projection intentionally strips defaults, examples, and
secret-looking values before they reach the renderer.

## Notes

- Frozen lockfile: use `pnpm install --frozen-lockfile` or `pnpm verify` for
  reproducible installs.
- Rust tests: `apps/palette-tauri/src-tauri` is isolated from the root Cargo
  workspace, so run its tests with the explicit manifest path.
- CSP: `style-src 'unsafe-inline'` is required because Tailwind v4 emits inline
  style blocks through the Vite plugin.
- Networking model: production renderer traffic goes through Tauri IPC only. In
  browser dev, `src/lib/invoke.ts` returns safe stubs for desktop-only commands.

Aurora tokens/components are rooted in:

- `src/components/aurora.css`
- `src/components/ui/aurora/*`
- `src/styles.css`

Components come from the `@aurora` shadcn registry configured in
`components.json`.
