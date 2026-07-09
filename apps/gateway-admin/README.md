# Gateway Admin UI

Static-exportable Next.js admin UI for the `gateway` surface, vendored into the `lab` repo at `apps/gateway-admin`.

The app is designed to be served as static assets while talking directly to the Rust gateway backend over HTTP. It can still be run in standalone frontend mode during development.

## Current State

- App framework: Next.js 16 + React 19
- Package manager: `pnpm` (lockfile included)
- Data mode: browser client over the Rust `/v1/gateway` endpoint, with same-origin browser session auth for hosted deployments and optional mock data for local UI work
- Security rule: browser-facing flows must use backend-supported redacted payloads for any response that can contain secrets. The Settings > Extract page requests redacted extract scan results and never receives raw extracted secret values.

## Local Usage

From this directory:

```bash
pnpm install
pnpm dev
```

The dev server binds on `0.0.0.0` so the UI is reachable from other devices on the local network during development.

The app defaults `NEXT_PUBLIC_API_URL` to `/v1`, which is the expected same-origin path once `labby serve` hosts both API and UI. Override it when pointing the UI at a different backend origin.

```bash
NEXT_PUBLIC_API_URL=http://127.0.0.1:8765/v1 pnpm dev
```

In hosted mode, the UI expects Rust-owned browser session auth:

- `GET /auth/session` boots the browser auth state
- `GET /auth/login` starts the Rust-owned login flow
- `POST /auth/logout` clears the browser session
- `/v1/*` uses same-origin requests with `credentials: 'include'`

For local binary-served UI work, keep the same-origin `/v1` path and start `labby serve` with web auth disabled for the browser surface only:

```bash
LAB_WEB_UI_AUTH_DISABLED=true \
LAB_MCP_HTTP_TOKEN=your-local-dev-token \
cargo run --bin labby -- serve --host 0.0.0.0 --port 8765
```

That mode keeps the MCP/backend token in place while making `/auth/session` and `/v1/*` immediately usable from the exported UI on the same origin. Hosted deployments should leave `LAB_WEB_UI_AUTH_DISABLED` unset so browser OAuth remains active. `LAB_WEB_UI_DISABLE_AUTH` is accepted as a legacy alias.

There is also a repo shortcut for that local no-auth mode:

```bash
just chat-local
```

Browser-facing bearer mode is intentionally disabled in the current UI. The gateway screens always use the Rust-owned browser session flow plus CSRF headers when talking to `/v1/*`. If you need a local-only backend bypass, use `LAB_WEB_UI_AUTH_DISABLED=true` on the Rust side rather than embedding a public browser token.

When the frontend and Rust backend run on different origins during local development, the backend must allow the frontend origin through CORS:

```bash
LAB_MCP_HTTP_TOKEN=your-local-dev-token \
LAB_CORS_ORIGINS=http://127.0.0.1:3101 \
cargo run --bin labby -- serve --host 0.0.0.0 --port 8765
```

```bash
NEXT_PUBLIC_API_URL=http://127.0.0.1:8765/v1 \
NEXT_PUBLIC_API_TOKEN=your-local-dev-token \
pnpm dev --hostname 127.0.0.1 --port 3101
```

## Static Export

Build the export artifact:

```bash
pnpm build
```

This writes the static site to `out/`.

Preview the exported assets locally:

```bash
pnpm start
```

## Module and Test Tooling Contract

This app is authored as a Next.js bundler-style ESM project. The canonical module
model is:

- `package.json` sets `"type": "module"`
- `tsconfig.json` uses `"module": "esnext"` and `"moduleResolution": "bundler"`
- absolute app imports use the `@/*` alias from `tsconfig.json`
- local relative imports may omit extensions when they are consumed by Next or
  the `tsx` test runner

The default verification path follows that same model:

```bash
pnpm test
```

`pnpm test` runs the Node-compatible unit suite through `tsx`, which honors the
same TypeScript and ESM assumptions used by the app. Browser-only checks remain
under `pnpm run test:browser`; they are intentionally separate from the default
unit gate.

Full `tsc --noEmit -p tsconfig.json` is not yet the default gate because older
UI surfaces still have unrelated strict-type debt. Do not add a package-level
`typecheck` script until that full command is green; otherwise contributors get
a verification command whose failures are unrelated to the module contract.

`lib/tooling-contract.test.ts` guards this contract so future script or
`tsconfig.json` changes do not silently split authoring, typecheck, and test
resolution semantics.

## Notes

- The imported UI code was originally developed as its own repository and is now tracked as normal source under this repo.
- Nested git metadata was removed on import so `apps/gateway-admin` behaves like a standard in-repo app directory.

