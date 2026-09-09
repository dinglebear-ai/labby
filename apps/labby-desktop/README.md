# Labby Desktop

Labby Desktop is a thin Tauri 2 shell for the authoritative Labby Control
Plane. It opens the same Gateway Admin application used in a browser; it does
not ship a second launcher, settings UI, authentication stack, or command
surface.

Until a route completes the Phabby migration gates, `apps/gateway-admin`
remains the authoritative web application. Its own Cmd-K command palette,
Aurora theme, browser session, CSRF handling, and `/settings/*` routes are
provided by that hosted application, not duplicated in the desktop shell.

## Native boundary

The native shell owns only:

- one standard decorated, resizable Control Plane window;
- a credential-free bootstrap Control Plane origin;
- a local loading and unavailable page;
- same-origin navigation and internal deep-link confinement;
- a bounded system-browser sign-in handoff to the hosted session;
- close-to-hide, macOS reopen, tray/app menu, packaging, and signing.

Remote content receives no privileged native command surface. Authentication
and product state remain in the hosted Control Plane and Labby/Depot backends.

## Sign-in boundary

The shell intercepts the configured origin's `/auth/login` navigation and opens
the system browser through that server's `/auth/desktop/authorize` endpoint.
It does not embed the identity provider's login page. The server must support
the desktop start, authorize, poll, and redeem endpoints; upgrading only the
desktop cannot enable this flow on an older server.

Each attempt uses a fresh PKCE proof and a bounded polling lifetime. Session
redemption occurs in the same-origin WebView, where the server sets its
HttpOnly browser-session cookie. The shell does not persist bearer tokens,
provider credentials, or the short-lived handoff secrets. Navigation and
replacement attempts invalidate obsolete in-flight work.

The app and tray menus provide **Open Control Plane**, **Settings** (an internal
`/settings/` deep link), and **Quit**.

## Configuration

The default origin is `http://localhost:8765`. Set
`LABBY_CONTROL_PLANE_URL` when building or launching against another Control
Plane. The platform config directory may contain a credential-free
`settings.json` with:

```json
{
  "controlPlaneUrl": "https://labby.example.com"
}
```

For upgrades, the reader imports only a validated `controlPlaneUrl` from the
former app's configuration directory. Malformed legacy configuration and
legacy API URLs are ignored. Bearer tokens, OAuth metadata, project IDs,
shortcuts, theme, and Palette preferences are never imported into the shell.

Only HTTP(S) origins are accepted. Non-loopback HTTP is rejected; production
origins must use HTTPS. Navigation is restricted to the configured origin, and
deep links must be absolute application paths without traversal or
scheme-relative forms.

## Development

```bash
pnpm install --frozen-lockfile
pnpm vite:build
pnpm verify
pnpm dev
pnpm build
cargo test --manifest-path apps/labby-desktop/src-tauri/Cargo.toml --locked
```

`pnpm vite:build` produces only the static local loader/fallback assets.
`pnpm build` creates the native release bundles. The Rust crate is isolated
from the root Cargo workspace, so native checks use its explicit manifest path.

Useful full native gates:

Keep Node.js available on `PATH` when running the Rust tests. The test suite
executes the generated WebView status and redemption scripts in a local Node
harness with mocked DOM and fetch behavior; these tests make no network calls.

```bash
cargo fmt --manifest-path apps/labby-desktop/src-tauri/Cargo.toml --check
cargo clippy --manifest-path apps/labby-desktop/src-tauri/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path apps/labby-desktop/src-tauri/Cargo.toml --locked
```

## Security and failure behavior

The shell first checks network reachability, then lets the WebView follow the
hosted application and its browser-session flow. HTTP status codes and redirects are not
treated as bootstrap failures because they may represent authentication. A
network failure, invalid origin, or load timeout restores the bundled local
error page; retrying from the app or tray menu starts a new generation so a
stale load cannot replace a newer one.

Do not add arbitrary HTTP IPC, bearer-token storage, a separate provider OAuth stack, filesystem
access, or backend business logic to this package. Those belong to the web
application or authoritative backend contracts.
