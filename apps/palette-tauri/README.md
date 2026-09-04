# Labby Desktop

Tauri v2 desktop control plane and command palette for a `labby serve`
instance. The full-size Control Plane window loads the canonical Gateway Admin
UI from the configured Control Plane origin, so every page uses the same project-bound
session, CSRF protection, routes, and static assets as the browser product. The
separate palette renderer is React with Aurora registry components; its Rust
bridge owns server URL resolution, OAuth/static bearer auth, and palette HTTP
traffic.

The Control Plane opens on app launch. The palette remains hidden until its
global shortcut or tray command is used. The tray can open either surface,
open palette settings, or quit. Closing either window hides it; only the
transient palette hides on blur.

## Control Plane Model

Tauri-command and deep-link inputs for the `control-plane` webview are limited
to absolute application paths on the saved Control Plane HTTP(S) origin.
Scheme-relative, traversal, and backslash inputs are rejected before the initial
navigation. Because Gateway Admin is served
by `labby serve`, the desktop app automatically exposes its complete route set,
including Gateways, Artifact Library, upstreams, access administration,
surfaces, logs, doctor, setup, and settings without maintaining a forked copy
of those pages in the palette bundle.

Use **Open Control Plane** in the palette footer or tray. The Tauri command also
accepts a validated internal path for future deep links, for example
`/skills/` or `/settings/surfaces/`.

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

## Desktop Smoke

`scripts/desktop-smoke.ps1` drives a built Windows palette from environment
configuration. It fetches `/v1/palette/catalog`, asserts the configured query
matches at least one launcher row, launches the app, types the query, captures a
screenshot, asserts the app process is still responding, and writes
`result.json`. The script closes the app by default; pass `-KeepApp` to leave it
open for debugging.

Copy `scripts/desktop-smoke.env.example` outside the repo and fill in local
values:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass `
  -File scripts/desktop-smoke.ps1 `
  -EnvFile C:\path\palette-smoke.env
```

API-only smoke can run from any Unix shell:

```bash
LABBY_PALETTE_ENV_FILE=/path/palette-smoke.env pnpm smoke:live
```

Remote Windows smoke is also env-driven:

```bash
LABBY_PALETTE_ENV_FILE=/path/palette-smoke.env pnpm smoke:agent-os
```

For a remote Windows desktop session, invoke it through an interactive scheduled
task (`schtasks /IT`) so keyboard input and screenshots target the visible
desktop.

## Configuration

The app reads Labby connection settings from environment defaults first:

- `LABBY_API_URL` (preferred; API origin that serves `/v1/palette/*`)
- `LABBY_CONTROL_PLANE_URL` (WebUI origin; defaults to `LABBY_API_URL`)
- `LABBY_MCP_HTTP_TOKEN`
- `LABBY_PROJECT_ID` (required for project-scoped Skill Library operations such as import)

`LABBY_PUBLIC_URL` is intentionally not used as a palette endpoint: it is the
OAuth issuer/callback base for the Labby server, not an API origin. Configure
`LABBY_API_URL` explicitly when the palette API is not on the local default. If
the configured origin returns HTML for `/v1/palette/catalog`, the bridge reports
a wrong-host configuration error instead of treating the web UI page as
catalog data. If the origin exposes `/.well-known/labby.json`, the bridge can
discover `apiBaseUrl` and retry palette catalog/execute calls against the
advertised API origin.

Runtime palette preferences are stored in the platform app config directory as
`settings.json`. The settings panel can independently override the API and
Control Plane origins, plus the static bearer token, shortcut, theme, result
layout, footer hints, and hide-on-blur behavior.

## Authentication

The palette authenticates to Labby two ways, and both can be configured:

- Static bearer token from `LABBY_MCP_HTTP_TOKEN` or the settings panel.
- OAuth "Sign in with Google" through the Rust shell's Authorization Code + PKCE
  flow.

When a valid OAuth credential exists for the active server, it takes precedence
over the static token. If OAuth is unavailable or expired, the bridge falls back
to the static token when configured.

OAuth refreshes are single-flight. Access and refresh tokens live in the
operating system credential vault; `oauth.json` contains only non-secret
metadata and the vault account identifier. A rotated token is published to the
in-memory cache only after both the vault secret and metadata are durable. If
the metadata write fails, the prior vault value is restored; if rollback also
fails, the bridge reports that credential state is uncertain instead of
pretending the old session is intact.

Legacy plaintext `oauth.json` credentials are migrated only after the vault
write succeeds. Sign-out revokes the server-side refresh grant before removing
the vault entry and metadata, and retains local credentials when revocation
fails so the user can retry safely. Vault read, write, or delete failures are
recoverable action errors and must be resolved through the platform credential
manager; deleting `oauth.json` alone does not erase its associated vault entry.
`settings.json` can still contain the optional static bearer token and retains
the restricted atomic-write and platform ACL protections.

## Schema Validation

The launcher catalog is intentionally compact: rows include
`schemaFingerprint`, but not the full input schema. The renderer lazily fetches
`/v1/palette/schema?id=<launcher-id>` when a row enters argument mode, then uses
Ajv for best-effort JSON Schema validation before submit, memoized by
`entry.id + schemaFingerprint`. Unknown or unsupported schemas fail open in the
renderer; the backend remains the authoritative validator for every execution.
Simple top-level object schemas render lightweight form controls that keep the
JSON payload synchronized; complex schemas stay in JSON mode.

The schema projection intentionally strips defaults, examples, and
secret-looking values before they reach the renderer.

## Search And Audit

The backend exposes `/v1/palette/search?q=<query>&limit=<n>` for server-side
filtering/ranking over compact launcher rows. The renderer also records the last
50 launches in local storage with redacted params so failed runs can be debugged
without leaking tokens or secrets.

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
