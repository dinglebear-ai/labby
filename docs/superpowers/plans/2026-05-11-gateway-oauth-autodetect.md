# Gateway OAuth Autodetect Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Automatically switch new custom HTTP Gateway entries from "No auth" to "OAuth (MCP)" when the existing OAuth probe discovers support, then attempt authorization and show a clear blocked-popup fallback.

**Architecture:** Keep the backend probe API unchanged. Add a small UI state extension for blocked auto-open, route successful probe results through one shared connect helper, and preserve manual auth selections by only auto-switching from `authMode === 'none'`. Tests cover the pure helper and the component behavior through the existing `node:test` + `happy-dom` frontend test style.

**Tech Stack:** Next.js Gateway Admin, React 19 hooks, TypeScript, SWR, `node:test`, `happy-dom`, existing `upstreamOauthApi`.

---

## File Structure

- Modify: `apps/gateway-admin/lib/types/upstream-oauth.ts`
  - Owns the shared `OAuthConnectState` union used by the Gateway dialog and upstream OAuth UI.
  - Add `{ kind: 'blocked'; upstream: string; issuer?: string; scopes?: string[] }` so the UI can distinguish "OAuth detected, browser blocked automatic popup" from generic errors.
- Modify: `apps/gateway-admin/components/gateway/gateway-form-dialog.tsx`
  - Owns the custom Gateway add/edit form, URL probe effect, OAuth connect button, and save validation.
  - Add pure exported helpers for auto-connect gating so tests do not need to reach into React internals.
  - Split the OAuth connect path so manual clicks still open `about:blank` synchronously, while auto-connect tries `window.open(authorization_url, '_blank')` after async probe/start and records `blocked` when it returns `null`.
- Create: `apps/gateway-admin/components/gateway/gateway-form-dialog.test.tsx`
  - Component-level tests using the repo's existing `node:test`, `happy-dom`, and React `act` pattern.
  - Verifies auto-switch, blocked fallback, manual bearer preservation, and edit-mode non-disruption.
- Optional modify after tests expose need: `apps/gateway-admin/components/chat/test-utils.tsx`
  - Only if the Gateway test needs a reusable DOM helper exported there. Prefer a local helper in the new test file first to avoid broad test utility churn.

## Task 1: Extend OAuth UI State

**Files:**
- Modify: `apps/gateway-admin/lib/types/upstream-oauth.ts`
- Test: `apps/gateway-admin/components/gateway/gateway-form-dialog.test.tsx` in Task 2 will exercise this state through the UI.

- [ ] **Step 1: Add the blocked OAuth state**

In `apps/gateway-admin/lib/types/upstream-oauth.ts`, replace the `OAuthConnectState` union with this exact union:

```ts
export type OAuthConnectState =
  | { kind: 'idle' }
  | { kind: 'probing' }
  | { kind: 'discovered'; upstream: string; issuer?: string; scopes?: string[] }
  | { kind: 'blocked'; upstream: string; issuer?: string; scopes?: string[] }
  | { kind: 'authorizing'; upstream: string }
  | { kind: 'connected'; upstream: string; registration_strategy: string; scopes?: string[] }
  | { kind: 'error'; message: string }
```

- [ ] **Step 2: Update `oauthUpstream` narrowing**

In `apps/gateway-admin/components/gateway/gateway-form-dialog.tsx`, update the `oauthUpstream` calculation so the new `blocked` state is allowed by the type guard:

```ts
  const oauthUpstream = oauthState.kind === 'authorizing'
    || oauthState.kind === 'connected'
    || oauthState.kind === 'discovered'
    || oauthState.kind === 'blocked'
    ? (oauthState as { upstream: string }).upstream
    : null
```

- [ ] **Step 3: Run type-focused frontend tests**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsc --noEmit
```

Expected: PASS, no TypeScript errors about `OAuthConnectState` exhaustiveness or missing `upstream` fields.

- [ ] **Step 4: Commit**

```bash
git add apps/gateway-admin/lib/types/upstream-oauth.ts apps/gateway-admin/components/gateway/gateway-form-dialog.tsx
git commit -m "feat(gateway-admin): add blocked oauth connect state"
```

## Task 2: Add Autodetect Helpers and Tests

**Files:**
- Modify: `apps/gateway-admin/components/gateway/gateway-form-dialog.tsx`
- Create: `apps/gateway-admin/components/gateway/gateway-form-dialog.test.tsx`

- [ ] **Step 1: Add pure helper exports**

In `apps/gateway-admin/components/gateway/gateway-form-dialog.tsx`, add these helpers below `serviceFields`:

```ts
export function shouldAutoConnectOauth({
  open,
  isEditing,
  transport,
  authMode,
  oauthDiscovered,
  upstream,
}: {
  open: boolean
  isEditing: boolean
  transport: TransportType
  authMode: GatewayAuthMode
  oauthDiscovered: boolean
  upstream?: string
}) {
  return open
    && !isEditing
    && transport === 'http'
    && authMode === 'none'
    && oauthDiscovered
    && !!upstream?.trim()
}

export function oauthConnectButtonLabel(state: OAuthConnectState) {
  if (state.kind === 'probing') return 'Detecting OAuth...'
  if (state.kind === 'authorizing') return 'Waiting...'
  if (state.kind === 'blocked') return 'Click to authorize'
  return 'Connect via OAuth'
}
```

Important: use ASCII `...` in the helper label. The existing component currently renders ellipses in some labels; converting the button text to the helper return value avoids brittle non-ASCII test matching.

- [ ] **Step 2: Write helper tests first**

Create `apps/gateway-admin/components/gateway/gateway-form-dialog.test.tsx` with:

```tsx
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  oauthConnectButtonLabel,
  shouldAutoConnectOauth,
} from './gateway-form-dialog'

test('shouldAutoConnectOauth only allows new HTTP no-auth OAuth discoveries', () => {
  assert.equal(shouldAutoConnectOauth({
    open: true,
    isEditing: false,
    transport: 'http',
    authMode: 'none',
    oauthDiscovered: true,
    upstream: 'github',
  }), true)

  assert.equal(shouldAutoConnectOauth({
    open: true,
    isEditing: false,
    transport: 'http',
    authMode: 'bearer',
    oauthDiscovered: true,
    upstream: 'github',
  }), false)

  assert.equal(shouldAutoConnectOauth({
    open: true,
    isEditing: true,
    transport: 'http',
    authMode: 'none',
    oauthDiscovered: true,
    upstream: 'github',
  }), false)

  assert.equal(shouldAutoConnectOauth({
    open: true,
    isEditing: false,
    transport: 'stdio',
    authMode: 'none',
    oauthDiscovered: true,
    upstream: 'github',
  }), false)

  assert.equal(shouldAutoConnectOauth({
    open: true,
    isEditing: false,
    transport: 'http',
    authMode: 'none',
    oauthDiscovered: true,
    upstream: '',
  }), false)
})

test('oauthConnectButtonLabel exposes blocked popup recovery copy', () => {
  assert.equal(oauthConnectButtonLabel({ kind: 'blocked', upstream: 'github' }), 'Click to authorize')
  assert.equal(oauthConnectButtonLabel({ kind: 'probing' }), 'Detecting OAuth...')
  assert.equal(oauthConnectButtonLabel({ kind: 'authorizing', upstream: 'github' }), 'Waiting...')
  assert.equal(oauthConnectButtonLabel({ kind: 'idle' }), 'Connect via OAuth')
})
```

- [ ] **Step 3: Run the new test and verify it fails before helper implementation**

If Step 1 was intentionally skipped to follow strict TDD, run now:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/gateway/gateway-form-dialog.test.tsx
```

Expected before Step 1: FAIL with missing exports.

If Step 1 already added the helpers, run the command anyway.

Expected after Step 1: PASS for both helper tests.

- [ ] **Step 4: Commit**

```bash
git add apps/gateway-admin/components/gateway/gateway-form-dialog.tsx apps/gateway-admin/components/gateway/gateway-form-dialog.test.tsx
git commit -m "test(gateway-admin): cover oauth autodetect gating"
```

## Task 3: Implement Auto-Connect Flow

**Files:**
- Modify: `apps/gateway-admin/components/gateway/gateway-form-dialog.tsx`
- Test: `apps/gateway-admin/components/gateway/gateway-form-dialog.test.tsx`

- [ ] **Step 1: Add refs for deduping auto-connect**

In `GatewayFormDialog`, near the existing refs, add:

```ts
  const autoOauthAttemptedForRef = useRef<string | null>(null)
```

In the existing dialog-open reset effect, in the new-server branch where the component resets `name`, `url`, and `authMode`, add:

```ts
        autoOauthAttemptedForRef.current = null
```

In the existing `useEffect(() => { setOauthState({ kind: 'idle' }); setOauthProbed(null) }, [url])`, change it to:

```ts
  useEffect(() => {
    if (skipUrlOauthResetRef.current) {
      skipUrlOauthResetRef.current = false
      return
    }
    autoOauthAttemptedForRef.current = null
    setOauthState({ kind: 'idle' })
    setOauthProbed(null)
  }, [url])
```

- [ ] **Step 2: Replace manual-only connect function with shared helper**

Replace `handleOauthConnect` with these two functions:

```ts
  async function runOauthConnect({
    authTab,
    auto,
    probeOverride,
  }: {
    authTab: Window | null
    auto: boolean
    probeOverride?: NonNullable<typeof oauthProbed>
  }) {
    if (!url.trim()) return
    setOauthState({ kind: 'probing' })
    try {
      const requestedUpstream = name.trim() || undefined
      const reusableProbe = probeOverride?.oauth_discovered
        && (!requestedUpstream || probeOverride.upstream === requestedUpstream)
      const probe = reusableProbe
        ? probeOverride
        : await upstreamOauthApi.probe(url.trim(), undefined, requestedUpstream)
      if (!probe.oauth_discovered) {
        authTab?.close()
        setOauthState({ kind: 'error', message: 'This server does not advertise OAuth support' })
        return
      }

      setOauthProbed(probe)
      setOauthState({ kind: 'discovered', upstream: probe.upstream, issuer: probe.issuer, scopes: probe.scopes })
      probeInfoRef.current = { registration_strategy: probe.registration_strategy ?? 'dynamic', scopes: probe.scopes }
      const { authorization_url } = await upstreamOauthApi.start(probe.upstream)

      const targetTab = authTab ?? window.open(authorization_url, '_blank')
      if (!targetTab || targetTab.closed) {
        setOauthState(
          auto
            ? { kind: 'blocked', upstream: probe.upstream, issuer: probe.issuer, scopes: probe.scopes }
            : { kind: 'error', message: 'Authorization tab was closed. Please try again.' },
        )
        return
      }

      if (authTab) {
        authTab.location.href = authorization_url
      }
      setOauthState({ kind: 'authorizing', upstream: probe.upstream })
    } catch (err: unknown) {
      authTab?.close()
      setOauthState({ kind: 'error', message: err instanceof Error ? err.message : 'OAuth connection failed' })
    }
  }

  async function handleOauthConnect() {
    if (!url.trim()) return
    const authTab = window.open('about:blank', '_blank')
    await runOauthConnect({ authTab, auto: false, probeOverride: oauthProbed ?? undefined })
  }
```

- [ ] **Step 3: Add auto-connect effect after the probe effect**

Add this effect after the existing URL probe effect:

```ts
  useEffect(() => {
    if (!oauthProbed) return
    if (!shouldAutoConnectOauth({
      open,
      isEditing,
      transport,
      authMode,
      oauthDiscovered: oauthProbed.oauth_discovered,
      upstream: oauthProbed.upstream,
    })) return

    const attemptKey = `${url.trim()}::${oauthProbed.upstream}`
    if (autoOauthAttemptedForRef.current === attemptKey) return
    autoOauthAttemptedForRef.current = attemptKey

    setAuthMode('oauth')
    void runOauthConnect({ authTab: null, auto: true, probeOverride: oauthProbed })
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [authMode, isEditing, oauthProbed, open, transport, url])
```

Do not include `runOauthConnect` in the dependency list unless it is wrapped in `useCallback`; this file already uses targeted hook dependency suppression for local form logic.

- [ ] **Step 4: Run the focused test command**

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/gateway/gateway-form-dialog.test.tsx
```

Expected: PASS for helper tests. Component behavior tests are added in Task 4.

- [ ] **Step 5: Commit**

```bash
git add apps/gateway-admin/components/gateway/gateway-form-dialog.tsx
git commit -m "feat(gateway-admin): auto-connect discovered oauth gateways"
```

## Task 4: Render Blocked Fallback UI and Component Tests

**Files:**
- Modify: `apps/gateway-admin/components/gateway/gateway-form-dialog.tsx`
- Modify: `apps/gateway-admin/components/gateway/gateway-form-dialog.test.tsx`

- [ ] **Step 1: Update OAuth panel copy and button styling**

In the `authMode === 'oauth'` panel, replace the message block before the error rendering with:

```tsx
                        <p className="text-sm text-aurora-text-muted">
                          {!url.trim()
                            ? 'Enter a URL above, then connect.'
                            : oauthState.kind === 'authorizing'
                              ? 'Complete authorization in the new tab.'
                              : oauthState.kind === 'blocked'
                                ? 'OAuth detected. Click to authorize; the browser blocked the automatic popup.'
                                : 'Connect this server via OAuth. A popup will open for you to authorize.'}
                        </p>
```

Replace the OAuth button with:

```tsx
                        <Button
                          type="button"
                          size="sm"
                          variant={oauthState.kind === 'blocked' ? 'default' : 'secondary'}
                          onClick={() => void handleOauthConnect()}
                          disabled={!url.trim() || oauthState.kind === 'probing' || oauthState.kind === 'authorizing'}
                          className={cn(
                            oauthState.kind === 'blocked'
                              && 'ring-2 ring-aurora-accent-primary/45 ring-offset-2 ring-offset-aurora-page-bg',
                          )}
                        >
                          {(oauthState.kind === 'probing' || oauthState.kind === 'authorizing') && (
                            <Loader2 className="size-4 mr-2 animate-spin" />
                          )}
                          {oauthConnectButtonLabel(oauthState)}
                        </Button>
```

- [ ] **Step 2: Add component test harness**

Append these helpers to `apps/gateway-admin/components/gateway/gateway-form-dialog.test.tsx` after the helper tests:

```tsx
import React from 'react'
import { act } from 'react'
import { createRoot } from 'react-dom/client'
import { SWRConfig } from 'swr'
import { Window } from 'happy-dom'

import { GatewayFormDialog } from './gateway-form-dialog'
import type { Gateway } from '@/lib/types/gateway'

function installGatewayDialogDom() {
  const window = new Window()
  Object.defineProperty(globalThis, 'window', { configurable: true, value: window })
  Object.defineProperty(globalThis, 'document', { configurable: true, value: window.document })
  Object.defineProperty(globalThis, 'navigator', { configurable: true, value: window.navigator })
  Object.defineProperty(globalThis, 'DOMException', { configurable: true, value: window.DOMException })
  Object.defineProperty(globalThis, 'Node', { configurable: true, value: window.Node })
  Object.defineProperty(globalThis, 'MouseEvent', { configurable: true, value: window.MouseEvent })
  Object.defineProperty(globalThis, 'PointerEvent', { configurable: true, value: window.PointerEvent })
  Object.defineProperty(globalThis, 'KeyboardEvent', { configurable: true, value: window.KeyboardEvent })
  Object.defineProperty(globalThis, 'Event', { configurable: true, value: window.Event })
  Object.defineProperty(globalThis, 'InputEvent', { configurable: true, value: window.InputEvent })
  Object.defineProperty(globalThis, 'HTMLElement', { configurable: true, value: window.HTMLElement })
  Object.defineProperty(globalThis, 'HTMLInputElement', { configurable: true, value: window.HTMLInputElement })
  Object.defineProperty(globalThis, 'IS_REACT_ACT_ENVIRONMENT', { configurable: true, value: true })
  return window
}

async function renderDialog(element: React.ReactElement) {
  const container = document.createElement('div')
  document.body.appendChild(container)
  const root = createRoot(container)
  await act(async () => {
    root.render(element)
  })
  return {
    container,
    unmount: async () => {
      await act(async () => root.unmount())
      container.remove()
    },
  }
}

async function waitFor(assertion: () => void) {
  const deadline = Date.now() + 2_000
  let lastError: unknown
  while (Date.now() < deadline) {
    try {
      assertion()
      return
    } catch (error) {
      lastError = error
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 20))
      })
    }
  }
  throw lastError
}

function renderOpenGatewayDialog(gateway: Gateway | null = null) {
  return renderDialog(
    <SWRConfig value={{ provider: () => new Map(), dedupingInterval: 0 }}>
      <GatewayFormDialog
        open
        onOpenChange={() => {}}
        gateway={gateway}
        onSave={async () => {}}
      />
    </SWRConfig>,
  )
}
```

- [ ] **Step 3: Add auto-switch blocked-popup test**

Append:

```tsx
test('new custom URL auto-switches to OAuth and shows blocked popup fallback', async () => {
  const window = installGatewayDialogDom()
  let openCalls = 0
  Object.defineProperty(window, 'open', {
    configurable: true,
    value: () => {
      openCalls += 1
      return null
    },
  })

  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (input, init) => {
    const path = String(input)
    if (path === '/v1/gateway' && init?.method === 'POST') {
      return new Response(JSON.stringify([]), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      })
    }
    if (path === '/v1/gateway/oauth/probe') {
      return new Response(JSON.stringify({
        upstream: 'github',
        url: 'https://github.example/mcp',
        oauth_discovered: true,
        issuer: 'https://github.example',
        scopes: ['mcp:read'],
        registration_strategy: 'dynamic',
      }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      })
    }
    if (path === '/v1/gateway/oauth/start') {
      return new Response(JSON.stringify({
        authorization_url: 'https://github.example/oauth/authorize',
      }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      })
    }
    return new Response(JSON.stringify([]), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    })
  }) as typeof fetch

  try {
    const view = await renderOpenGatewayDialog()
    const urlInput = document.querySelector('#url') as HTMLInputElement | null
    assert.ok(urlInput)

    await act(async () => {
      const setValue = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set
      setValue?.call(urlInput, 'https://github.example/mcp')
      urlInput.dispatchEvent(new window.InputEvent('input', { bubbles: true, data: 'https://github.example/mcp' }) as unknown as Event)
      await new Promise((resolve) => setTimeout(resolve, 700))
    })

    await waitFor(() => {
      assert.match(document.body.textContent ?? '', /OAuth \(MCP\)/)
      assert.match(document.body.textContent ?? '', /OAuth detected\. Click to authorize; the browser blocked the automatic popup\./)
      assert.match(document.body.textContent ?? '', /Click to authorize/)
      assert.equal(openCalls, 1)
    })

    await view.unmount()
  } finally {
    globalThis.fetch = originalFetch
  }
})
```

- [ ] **Step 4: Add bearer preservation helper-level test**

The component uses Radix Select, which is expensive to drive in `happy-dom`. Keep this path covered through the pure helper from Task 2:

```tsx
test('manual bearer mode prevents OAuth auto-connect even when probe discovers OAuth', () => {
  assert.equal(shouldAutoConnectOauth({
    open: true,
    isEditing: false,
    transport: 'http',
    authMode: 'bearer',
    oauthDiscovered: true,
    upstream: 'github',
  }), false)
})
```

- [ ] **Step 5: Add edit-mode non-disruption helper-level test**

```tsx
test('editing an existing gateway prevents OAuth auto-connect', () => {
  assert.equal(shouldAutoConnectOauth({
    open: true,
    isEditing: true,
    transport: 'http',
    authMode: 'none',
    oauthDiscovered: true,
    upstream: 'github',
  }), false)
})
```

- [ ] **Step 6: Run focused frontend tests**

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/gateway/gateway-form-dialog.test.tsx lib/api/upstream-oauth-client.test.ts
```

Expected: PASS. The new component test should show one `window.open` call and blocked fallback copy in the rendered dialog.

- [ ] **Step 7: Commit**

```bash
git add apps/gateway-admin/components/gateway/gateway-form-dialog.tsx apps/gateway-admin/components/gateway/gateway-form-dialog.test.tsx
git commit -m "feat(gateway-admin): show blocked oauth popup fallback"
```

## Task 5: Full Verification

**Files:**
- No code changes expected.

- [ ] **Step 1: Run Gateway Admin unit suite**

```bash
pnpm --dir apps/gateway-admin test
```

Expected: PASS. This covers `components/**/*.test.tsx` and `lib/**/*.test.ts`.

- [ ] **Step 2: Run lint for the frontend app**

```bash
pnpm --dir apps/gateway-admin lint
```

Expected: PASS. If lint flags hook dependencies in the auto-connect effect, either wrap `runOauthConnect` in `useCallback` with a complete dependency list or keep the existing targeted `react-hooks/exhaustive-deps` suppression immediately above the effect.

- [ ] **Step 3: Run Rust/API regression slice if frontend save behavior changed**

Run this if implementation touched backend files or changed request payloads:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features upstream_oauth gateway
```

Expected: PASS. If no backend files or API payload shapes changed, record that this command was skipped because the implementation is frontend-only and the existing `upstreamOauthApi.probe`/`start` contracts were preserved.

- [ ] **Step 4: Manual browser smoke**

Start the app using the repo's normal dev flow:

```bash
pnpm --dir apps/gateway-admin dev
```

In a browser:

1. Open Gateway Admin.
2. Add a custom HTTP server.
3. Enter an OAuth-capable MCP URL.
4. Confirm "Authentication" switches from "No auth" to "OAuth (MCP)".
5. Confirm one of these outcomes:
   - Popup opens to the authorization URL and the button says "Waiting...".
   - Browser blocks the popup and the OAuth section says "OAuth detected. Click to authorize; the browser blocked the automatic popup."
6. Select "Bearer token", change the URL to an OAuth-capable URL, and confirm the mode remains "Bearer token".
7. Edit an existing server and confirm loading the dialog does not auto-open OAuth.

- [ ] **Step 5: Final commit if verification required fixes**

Only run if Step 1-4 required follow-up edits:

```bash
git add apps/gateway-admin/components/gateway/gateway-form-dialog.tsx apps/gateway-admin/components/gateway/gateway-form-dialog.test.tsx apps/gateway-admin/lib/types/upstream-oauth.ts
git commit -m "fix(gateway-admin): stabilize oauth autodetect verification"
```

## Self-Review Notes

- Spec coverage:
  - Auto-switch from "No auth" to "OAuth (MCP)": Task 3 auto-connect effect calls `setAuthMode('oauth')` only when `shouldAutoConnectOauth` allows it.
  - Preserve manual selections: Task 2 helper and Task 4 tests lock out `authMode: 'bearer'`.
  - Auto-open authorization popup: Task 3 calls `window.open(authorization_url, '_blank')` for auto flow after `gateway.oauth.start`.
  - Blocked popup fallback: Task 1 adds `blocked`; Task 4 renders fallback copy and highlighted button.
  - Edit path: Task 2 and Task 4 helper tests require `isEditing === false`.
- Placeholder scan: No forbidden placeholder phrases or unspecified test instructions are intentionally left in implementation steps.
- Type consistency:
  - `OAuthConnectState.kind === 'blocked'` is defined before use.
  - `shouldAutoConnectOauth` receives existing local types `TransportType` and `GatewayAuthMode`.
  - `oauthConnectButtonLabel` receives `OAuthConnectState` from the shared type file.
