# CLAUDE.md — Labby Desktop

Contributor guide for `apps/labby-desktop`, the thin native shell around the
authoritative Labby Control Plane. Read `README.md` and the repository-root
`CLAUDE.md` before changing this package.

## Product boundary

- `apps/gateway-admin` (and eventually parity-gated Phabby routes) owns all
  product UI, Cmd-K commands, authentication, theme, and settings.
- This package owns one standard Control Plane window, local load/error assets,
  same-origin navigation confinement, minimal credential-free origin bootstrap,
  native lifecycle, and packaging.
- Never recreate the retired standalone Palette renderer, global launcher
  shortcut, bearer-token/native-OAuth store, action bridge, or duplicate
  settings surface here.
- Remote WebView content receives no native capability by default. Any new IPC
  requires an explicit narrow origin and command allowlist plus tests.

## Files

- `public/control-plane-loader.html` — bundled local loading/error shell.
- `src-tauri/src/lib.rs` — window, navigation, load generation, menus, and
  platform lifecycle.
- `src-tauri/src/persistence.rs` — credential-free Control Plane origin only,
  including legacy origin migration.
- `src-tauri/tauri.conf.json` — desktop window and bundle identity.

The desktop version is independent of the root Rust workspace. Keep it aligned
across `package.json`, `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`, and
`src-tauri/tauri.conf.json`.

## Verification

```bash
pnpm install --frozen-lockfile
pnpm vite:build
pnpm verify
cargo fmt --manifest-path apps/labby-desktop/src-tauri/Cargo.toml --check
cargo clippy --manifest-path apps/labby-desktop/src-tauri/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path apps/labby-desktop/src-tauri/Cargo.toml --locked
```

For desktop releases, additionally build the platform bundle and prove the
installed app loads the intended origin, confines deep links, recovers from an
unavailable origin, hides/reopens correctly, and preserves only the non-secret
bootstrap origin across upgrades.
