# CLAUDE.md — Labby Palette Tauri

Contributor guide for `apps/palette-tauri`, the desktop command palette for a
`labby serve` instance. Read `README.md` first for the runtime/security model.

The palette is versioned independently from the CLI (`package.json` /
`tauri.conf.json` carry the palette version). Do not sync it to the workspace
Cargo version.

## Architecture

- `src/App.tsx` is the stateful orchestrator. It owns query, selection, active
  launcher entry, argument mode, settings overlays, and run state.
- Business logic belongs in `src/lib/*`, not components.
- Presentational components live in `src/components/`.

Important launcher files:

- `src/lib/labbyClient.ts` — typed client wrappers over the Tauri bridge.
- `src/lib/launcherCatalog.ts` — launcher entry normalization, search, and
  catalog hook.
- `src/lib/launcherValidation.ts` — Ajv best-effort validation and secret-param
  redaction.
- `src-tauri/src/labby_bridge.rs` — fixed Labby HTTP bridge commands with
  request validation and reauth.

## Transport Rule

Renderer code must not speak MCP or arbitrary HTTP directly. Every backend call
goes through `src/lib/invoke.ts`, which forwards to fixed Tauri commands in
production. The browser dev fallback may return safe stubs, but it must not make
launcher execution look successful.

Do not introduce Axon/OpenAPI generated clients here. Use Labby backend
contracts and the bridge commands:

- `fetch_catalog`
- `dispatch_action`
- `fetch_launcher_catalog`
- `execute_launcher_entry`

## Launcher Rules

- Catalog entries are display hints only. Execution must go through
  `/v1/palette/execute`, which re-resolves live backend state.
- Keep secret-looking params out of retry/history/debug state. Use
  `redactLauncherParams`.
- Frontend JSON Schema validation is advisory. Backend validation is final.
- Preserve stale-result guards for long-running calls; an old response must not
  overwrite a newer run state.

## Test Convention

TypeScript uses co-located `*.test.ts(x)` files. Component tests use
`@testing-library/react`; pure helpers use Vitest.

Rust sidecar tests live in `src-tauri` and run explicitly:

```bash
cargo test --manifest-path apps/palette-tauri/src-tauri/Cargo.toml
```

## Design System

Use Aurora tokens and primitives. Do not introduce raw hex colors or duplicate
interactive primitives. Existing Aurora files:

- `src/components/aurora.css`
- `src/components/ui/aurora/*`
- `src/styles.css`
