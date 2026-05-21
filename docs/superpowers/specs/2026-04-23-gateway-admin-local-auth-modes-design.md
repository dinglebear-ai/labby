# Gateway Admin Local Auth Modes Design

## Goal

Support three explicit local review modes for `apps/gateway-admin` without changing production auth behavior:

1. mock data
2. real backend + local auth bypass
3. real backend + real auth

## Design

### Auth mode contract

Add a development-only local auth bypass mode to the existing auth mode layer.

- `NEXT_PUBLIC_MOCK_DATA=true` keeps the current mock preview behavior.
- `NEXT_PUBLIC_LOCAL_AUTH_BYPASS=true` enables real backend access while bypassing hosted browser-session bootstrapping locally.
- hosted auth remains the default when neither flag is set.

The local bypass must be gated to development so production builds cannot accidentally ship with bypass enabled.

### Session behavior

The local bypass must not only skip the login screen. It must also provide a stable authenticated browser-session shape because the sidebar and activity surfaces read `session.user`.

In local bypass mode, the frontend should expose a synthetic authenticated session with:

- deterministic `sub`
- deterministic `email`
- deterministic `csrfToken`
- long-lived `expiresAt`

This keeps UI consumers stable while the Rust backend runs with local web auth disabled.

### Backend pairing

The expected real-backend local pairing remains:

- frontend dev server points at `NEXT_PUBLIC_API_URL=http://127.0.0.1:8765/v1`
- Rust backend runs with `LAB_WEB_UI_DISABLE_AUTH=true`

This keeps browser-origin traffic real while removing local auth friction.

### Settings visibility

The Settings page should report local bypass distinctly rather than labeling it as hosted browser session auth.

Target labels:

- auth mode: `Local dev bypass`
- runtime: `Live control plane` for real backend paths
- runtime: `Mock preview` for mock mode

### Documentation

`apps/gateway-admin/README.md` should document the three local workflows explicitly and make `real backend + local auth bypass` the recommended default for day-to-day UI work.

## Files

- Modify `apps/gateway-admin/lib/auth/auth-mode.ts`
- Modify `apps/gateway-admin/lib/auth/session-store.ts`
- Modify `apps/gateway-admin/app/(admin)/settings/page.tsx`
- Modify `apps/gateway-admin/lib/dashboard/admin-insights.ts`
- Modify `apps/gateway-admin/README.md`

## Non-goals

- no production auth changes
- no backend auth protocol redesign
- no standalone browser bearer mode
