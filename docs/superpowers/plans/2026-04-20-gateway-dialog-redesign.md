# Gateway Dialog Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Redesign the Add/Edit Gateway dialog in `gateway-admin` to add a brand-icon Lab Service picker, ENV and JSON slide-out drawers, an auth dropdown, a `proxy_prompts` toggle, and the supporting type changes.

**Architecture:** `DialogContent` gets `overflow-visible` to let absolute-positioned drawers extend right of the dialog shell. Brand-colored icon backgrounds with white Simple Icons CDN logos handle 14 services; inline SVGs handle 7. Two-way form↔JSON sync uses a `syncingRef` boolean guard with deferred reset (`setTimeout(..., 0)`) to survive React's batched state flush.

**Tech Stack:** Next.js / React, Tailwind CSS, shadcn/ui (Radix Dialog, Select), TypeScript, `useSupportedServices()` hook for service data.

---

## File Map

| Action | File |
|--------|------|
| Modify | `apps/gateway-admin/lib/types/gateway.ts` |
| Modify | `apps/gateway-admin/lib/server/gateway-adapter.ts` |
| Modify | `apps/gateway-admin/components/gateway/gateway-form-dialog.tsx` |
| Modify | `apps/gateway-admin/components/gateway/gateway-list-content.tsx` |
| Modify | `apps/gateway-admin/components/gateway/gateway-detail-content.tsx` |
| Delete | `apps/gateway-admin/components/gateway/lab-service-picker.tsx` |

---

## Task 1: Add `proxy_prompts` to the type system

**Files:**
- Modify: `apps/gateway-admin/lib/types/gateway.ts:5-13`
- Modify: `apps/gateway-admin/lib/server/gateway-adapter.ts`

- [ ] **Step 1: Add `proxy_prompts` to `GatewayConfig`**

In `lib/types/gateway.ts`, add `proxy_prompts` immediately after `proxy_resources`:

```typescript
export interface GatewayConfig {
  url?: string
  command?: string
  args?: string[]
  bearer_token_env?: string
  oauth_enabled?: boolean
  proxy_resources?: boolean
  proxy_prompts?: boolean   // ← add this line
  expose_tools?: string[]
}
```

- [ ] **Step 2: Thread `proxy_prompts` through the gateway adapter**

`proxy_resources` appears in `lib/server/gateway-adapter.ts` at exactly these lines (verified by grep):

- **Line 65** — `GatewayConfigRaw` interface: add `proxy_prompts?: boolean` directly below `proxy_resources?: boolean`
- **Line 315** — build-default path: add `proxy_prompts: false,` directly below `proxy_resources: false,`
- **Line 370** — read path (API → frontend): add `proxy_prompts: config.proxy_prompts,` directly below `proxy_resources: config.proxy_resources,`
- **Line 436** — normalize path: add `proxy_prompts: config.proxy_prompts ?? false,` directly below `proxy_resources: config.proxy_resources ?? false,`
- **Line 498** — write path (frontend → API): add `proxy_prompts: input.config.proxy_prompts ?? false,` directly below `proxy_resources: input.config.proxy_resources ?? false,`
- **Lines 558–559** — patch path: after `patch.proxy_resources = config.proxy_resources`, add:
  ```typescript
  if (config.proxy_prompts !== undefined) {
    patch.proxy_prompts = config.proxy_prompts
  }
  ```

- [ ] **Step 3: Run the TypeScript compiler to verify no new errors**

```bash
cd apps/gateway-admin && npx tsc --noEmit 2>&1 | head -40
```

Expected: zero new errors referencing `proxy_prompts` (there may be pre-existing unrelated errors).

- [ ] **Step 4: Commit**

```bash
rtk git add apps/gateway-admin/lib/types/gateway.ts apps/gateway-admin/lib/server/gateway-adapter.ts
rtk git commit -m "feat(gateway): add proxy_prompts field to GatewayConfig and adapter"
```

---

## Task 2: Inline the service grid with brand icons (replace LabServicePicker)

**Files:**
- Modify: `apps/gateway-admin/components/gateway/gateway-form-dialog.tsx`
- Delete: `apps/gateway-admin/components/gateway/lab-service-picker.tsx`

The `LabServicePicker` component is replaced by an inline 3-column grid with brand-colored icon backgrounds. Simple Icons CDN provides white PNGs for 14 services; inline SVGs handle the remaining 7 (Apprise, Arcane, ByteStash, Gotify, Linkding, Memos, TEI).

- [ ] **Step 1: Add brand-color and logo constants near the top of `gateway-form-dialog.tsx` (after imports, before the component)**

```typescript
const SERVICE_BRANDS: Record<string, string> = {
  apprise: '#3B7BBF',
  arcane: '#0DB7ED',
  bytestash: '#6B73FF',
  gotify: '#45AEE5',
  linkding: '#7C5CBF',
  memos: '#3478F6',
  openai: '#10A37F',
  overseerr: '#E5870A',
  paperless: '#17BC6C',
  plex: '#CC7B19',
  prowlarr: '#F16529',
  qbittorrent: '#2F99E0',
  qdrant: '#DC244C',
  radarr: '#F0BC40',
  sabnzbd: '#F4A623',
  sonarr: '#35C5F4',
  tailscale: '#1E5EFF',
  tautulli: '#D9A21B',
  tei: '#FF9D00',
  unifi: '#0559C9',
  unraid: '#F45B00',
}

const siw = (slug: string) => `https://cdn.simpleicons.org/${slug}/ffffff`

const SERVICE_LOGOS: Record<string, string | null> = {
  apprise: null,
  arcane: null,
  bytestash: null,
  gotify: null,
  linkding: null,
  memos: null,
  tei: null,
  openai: siw('openai'),
  overseerr: siw('overseerr'),
  paperless: siw('paperlessngx'),
  plex: siw('plex'),
  prowlarr: siw('prowlarr'),
  qbittorrent: siw('qbittorrent'),
  qdrant: siw('qdrant'),
  radarr: siw('radarr'),
  sabnzbd: siw('sabnzbd'),
  sonarr: siw('sonarr'),
  tailscale: siw('tailscale'),
  tautulli: siw('tautulli'),
  unifi: siw('ubiquiti'),
  unraid: siw('unraid'),
}

// White inline SVGs for services not in Simple Icons
const SERVICE_SVG_FALLBACKS: Record<string, string> = {
  apprise: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm1 15h-2v-6h2v6zm0-8h-2V7h2v2z"/></svg>`,
  arcane: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M21 4.5l-9-2.25L3 4.5v9c0 5.55 3.84 10.74 9 12 5.16-1.26 9-6.45 9-12v-9z"/></svg>`,
  bytestash: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M20 6H4V4h16v2zm0 2H4v2h16V8zm0 4H4v2h16v-2zm0 4H4v2h16v-2z"/></svg>`,
  gotify: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M20 2H4c-1.1 0-2 .9-2 2v18l4-4h14c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2zm0 14H6l-2 2V4h16v12z"/></svg>`,
  linkding: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M17 7h-4v2h4c1.65 0 3 1.35 3 3s-1.35 3-3 3h-4v2h4c2.76 0 5-2.24 5-5s-2.24-5-5-5zm-6 8H7c-1.65 0-3-1.35-3-3s1.35-3 3-3h4V7H7c-2.76 0-5 2.24-5 5s2.24 5 5 5h4v-2zm-3-4h8v2H8v-2z"/></svg>`,
  memos: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M14 2H6c-1.1 0-2 .9-2 2v16c0 1.1.9 2 2 2h12c1.1 0 2-.9 2-2V8l-6-6zm2 16H8v-2h8v2zm0-4H8v-2h8v2zm-3-5V3.5L18.5 9H13z"/></svg>`,
  tei: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M21 3H3v2h9v14h2V5h9V3zM5 9v2h3v8h2v-8h3V9H5z"/></svg>`,
}
```

- [ ] **Step 2: Add `ServiceIconBox` helper component (above `GatewayFormDialog`)**

```typescript
function ServiceIconBox({ serviceKey }: { serviceKey: string }) {
  const brand = SERVICE_BRANDS[serviceKey] ?? '#1d3d4e'
  const logo = SERVICE_LOGOS[serviceKey]
  const svg = SERVICE_SVG_FALLBACKS[serviceKey]

  return (
    <div
      className="flex items-center justify-center w-9 h-9 rounded-lg shrink-0"
      style={{ background: `${brand}CC`, border: `1px solid ${brand}` }}
    >
      {logo ? (
        <img src={logo} alt="" className="w-5 h-5 object-contain" />
      ) : svg ? (
        <span
          className="w-5 h-5 block"
          // biome-ignore lint/security/noDangerouslySetInnerHtml: trusted static SVG strings
          dangerouslySetInnerHTML={{ __html: svg }}
        />
      ) : (
        <span className="text-white text-xs font-bold">{serviceKey[0]?.toUpperCase()}</span>
      )}
    </div>
  )
}
```

- [ ] **Step 3: Replace the `LabServicePicker` in the Lab tab with an inline grid**

In `gateway-form-dialog.tsx`, inside `<TabsContent value="lab">`, replace:

```typescript
<LabServicePicker
  selectedService={selectedService}
  services={supportedServices ?? []}
  onSelect={setSelectedService}
/>
```

with:

```typescript
<div
  className="grid grid-cols-2 sm:grid-cols-3 gap-2 overflow-y-auto pr-1"
  style={{ maxHeight: 320 }}
>
  {(supportedServices ?? []).map((svc) => (
    <button
      key={svc.key}
      type="button"
      onClick={() => setSelectedService(svc.key)}
      className={cn(
        'flex flex-col gap-2 rounded-xl border p-3 text-left transition-colors hover:border-primary/60 hover:bg-accent/30',
        selectedService === svc.key
          ? 'border-primary bg-primary/10'
          : 'border-border bg-background',
      )}
    >
      <ServiceIconBox serviceKey={svc.key} />
      <div className="min-w-0">
        <p className="text-sm font-medium leading-tight truncate">{svc.display_name}</p>
        <p className="text-xs text-muted-foreground mt-0.5 truncate">{svc.category}</p>
        <p className="text-xs text-muted-foreground mt-1 line-clamp-2 leading-snug">{svc.description}</p>
      </div>
    </button>
  ))}
</div>
```

- [ ] **Step 4: Remove the `LabServicePicker` import from `gateway-form-dialog.tsx`**

Delete this line near the top:
```typescript
import { LabServicePicker } from './lab-service-picker'
```

- [ ] **Step 5: Delete the now-unused component file**

```bash
rm apps/gateway-admin/components/gateway/lab-service-picker.tsx
```

- [ ] **Step 6: Verify no remaining imports of `LabServicePicker`**

```bash
grep -r "lab-service-picker\|LabServicePicker" apps/gateway-admin/
```

Expected: no output.

- [ ] **Step 7: TypeScript check**

```bash
cd apps/gateway-admin && npx tsc --noEmit 2>&1 | grep -E "error TS" | head -20
```

Expected: no new errors from the files we touched.

- [ ] **Step 8: Commit**

```bash
rtk git add apps/gateway-admin/components/gateway/gateway-form-dialog.tsx
rtk git rm apps/gateway-admin/components/gateway/lab-service-picker.tsx
rtk git commit -m "feat(gateway): inline brand-icon service grid, remove LabServicePicker"
```

---

## Task 3: Add `proxyPrompts` toggle and drawer state

**Files:**
- Modify: `apps/gateway-admin/components/gateway/gateway-form-dialog.tsx`

- [ ] **Step 1: Add new state variables after the existing `proxyResources` state**

In `gateway-form-dialog.tsx`, after:
```typescript
const [proxyResources, setProxyResources] = useState(true)
```

add:
```typescript
const [proxyPrompts, setProxyPrompts] = useState(true)
const [envDrawerOpen, setEnvDrawerOpen] = useState(false)
const [jsonDrawerOpen, setJsonDrawerOpen] = useState(false)
const [jsonText, setJsonText] = useState('')
const [jsonValid, setJsonValid] = useState(false)
const syncingRef = useRef(false)
```

The `syncingRef` is a ref (not state) so that toggling it doesn't trigger re-renders.

- [ ] **Step 2: Add `proxyPrompts` to `emptyCustomState`**

Change:
```typescript
const emptyCustomState = {
  transport: 'http' as TransportType,
  name: '',
  url: '',
  command: '',
  args: '',
  bearerTokenEnv: '',
  proxyResources: true,
}
```

to:
```typescript
const emptyCustomState = {
  transport: 'http' as TransportType,
  name: '',
  url: '',
  command: '',
  args: '',
  bearerTokenEnv: '',
  proxyResources: true,
  proxyPrompts: true,
}
```

- [ ] **Step 3: Initialize `proxyPrompts` from gateway on edit open**

In the `useEffect` that handles `open` changing, find where `setProxyResources(gateway.config.proxy_resources ?? true)` is set and add directly below it:
```typescript
setProxyPrompts(gateway.config.proxy_prompts ?? true)
```

Also in the `else` branch (creating new gateway), find:
```typescript
setProxyResources(emptyCustomState.proxyResources)
```
and add:
```typescript
setProxyPrompts(emptyCustomState.proxyPrompts)
```

- [ ] **Step 4: Close drawers when mode (tab) changes**

Find the existing `onValueChange` on the `Tabs` component:
```typescript
onValueChange={(value) => setMode(value as FormMode)}
```

Change to:
```typescript
onValueChange={(value) => {
  setMode(value as FormMode)
  setEnvDrawerOpen(false)
  setJsonDrawerOpen(false)
}}
```

- [ ] **Step 5: Add the Proxy Prompts toggle below the Proxy Resources toggle in the Custom tab**

Find the Proxy Resources div block in `<TabsContent value="custom">`:
```typescript
<div className="flex items-center justify-between rounded-lg border p-4">
  <div className="space-y-0.5">
    <Label htmlFor="proxy-resources" className="font-medium">
      Proxy Resources
    </Label>
    ...
  </div>
  <Switch id="proxy-resources" checked={proxyResources} onCheckedChange={setProxyResources} />
</div>
```

Add this block immediately after it:
```typescript
<div className="flex items-center justify-between rounded-lg border p-4">
  <div className="space-y-0.5">
    <Label htmlFor="proxy-prompts" className="font-medium">
      Proxy Prompts
    </Label>
    <p className="text-sm text-muted-foreground">
      Forward MCP prompt requests to this gateway
    </p>
  </div>
  <Switch
    id="proxy-prompts"
    checked={proxyPrompts}
    onCheckedChange={setProxyPrompts}
  />
</div>
```

- [ ] **Step 6: Add `proxy_prompts` to `buildInput()`**

Find in `buildInput()`:
```typescript
proxy_resources: proxyResources,
```

Add after it:
```typescript
proxy_prompts: proxyPrompts,
```

- [ ] **Step 7: TypeScript check**

```bash
cd apps/gateway-admin && npx tsc --noEmit 2>&1 | grep "error TS" | head -20
```

- [ ] **Step 8: Commit**

```bash
rtk git add apps/gateway-admin/components/gateway/gateway-form-dialog.tsx
rtk git commit -m "feat(gateway): add proxyPrompts toggle and drawer state"
```

---

## Task 4: ENV and JSON chip buttons in dialog header

**Files:**
- Modify: `apps/gateway-admin/components/gateway/gateway-form-dialog.tsx`

The ENV and JSON chips appear in the `DialogHeader` right slot, visible only on the Custom tab. Clicking an active chip closes it (toggle behavior). Opening one closes the other.

- [ ] **Step 1: Add helper functions for toggling drawers (above the return statement)**

```typescript
const toggleEnvDrawer = () => {
  const next = !envDrawerOpen
  setEnvDrawerOpen(next)
  if (next) setJsonDrawerOpen(false)
}

const toggleJsonDrawer = () => {
  const next = !jsonDrawerOpen
  setJsonDrawerOpen(next)
  if (next) setEnvDrawerOpen(false)
}
```

- [ ] **Step 2: Restructure the `DialogHeader` to support a right-side chip row**

Replace the current `DialogHeader` block:
```typescript
<DialogHeader className="shrink-0">
  <DialogTitle>{isEditing ? 'Edit Gateway' : 'Add Gateway'}</DialogTitle>
  <DialogDescription>
    {isEditing
      ? 'Edit gateway settings.'
      : mode === 'lab'
        ? 'Connect a built-in Lab service.'
        : 'Connect an upstream MCP server.'}
  </DialogDescription>
</DialogHeader>
```

with:
```typescript
<DialogHeader className="shrink-0">
  <div className="flex items-start justify-between gap-2">
    <div className="flex flex-col gap-1">
      <DialogTitle>{isEditing ? 'Edit Gateway' : 'Add Gateway'}</DialogTitle>
      <DialogDescription>
        {isEditing
          ? 'Edit gateway settings.'
          : mode === 'lab'
            ? 'Connect a built-in Lab service.'
            : 'Connect an upstream MCP server.'}
      </DialogDescription>
    </div>
    <div
      className="flex gap-1.5 shrink-0"
      style={{ visibility: mode === 'custom' ? 'visible' : 'hidden' }}
    >
      <button
        type="button"
        onClick={toggleEnvDrawer}
        className={cn(
          'rounded-full border px-3 py-1 text-xs font-medium transition-colors',
          envDrawerOpen
            ? 'border-primary bg-primary text-primary-foreground'
            : 'border-border bg-background text-foreground hover:bg-accent',
        )}
      >
        ENV
      </button>
      <button
        type="button"
        onClick={toggleJsonDrawer}
        className={cn(
          'rounded-full border px-3 py-1 text-xs font-medium transition-colors',
          jsonDrawerOpen
            ? 'border-primary bg-primary text-primary-foreground'
            : 'border-border bg-background text-foreground hover:bg-accent',
        )}
      >
        JSON
      </button>
    </div>
  </div>
</DialogHeader>
```

- [ ] **Step 3: TypeScript check**

```bash
cd apps/gateway-admin && npx tsc --noEmit 2>&1 | grep "error TS" | head -20
```

- [ ] **Step 4: Commit**

```bash
rtk git add apps/gateway-admin/components/gateway/gateway-form-dialog.tsx
rtk git commit -m "feat(gateway): add ENV/JSON chip buttons to dialog header"
```

---

## Task 5: ENV drawer

**Files:**
- Modify: `apps/gateway-admin/components/gateway/gateway-form-dialog.tsx`
- Modify: `apps/gateway-admin/components/ui/dialog.tsx`

The ENV drawer slides out 300px to the right of the dialog. The dialog gets `overflow-visible` so the drawer isn't clipped. The drawer is `position: absolute; top: 0; left: 100%; bottom: 0` with a width transition.

- [ ] **Step 1: Add service prefix detection constant (top of file, after imports)**

```typescript
const SERVICE_ENV_PREFIXES: Record<string, string> = {
  APPRISE: 'apprise',
  ARCANE: 'arcane',
  BYTESTASH: 'bytestash',
  GOTIFY: 'gotify',
  LINKDING: 'linkding',
  MEMOS: 'memos',
  OPENAI: 'openai',
  OVERSEERR: 'overseerr',
  PAPERLESS: 'paperless',
  PLEX: 'plex',
  PROWLARR: 'prowlarr',
  QBITTORRENT: 'qbittorrent',
  QDRANT: 'qdrant',
  RADARR: 'radarr',
  SABNZBD: 'sabnzbd',
  SONARR: 'sonarr',
  TAILSCALE: 'tailscale',
  TAUTULLI: 'tautulli',
  TEI: 'tei',
  UNIFI: 'unifi',
  UNRAID: 'unraid',
}

function parseEnvText(text: string): { pairs: Record<string, string>; detectedServices: string[] } {
  const pairs: Record<string, string> = {}
  for (const line of text.split('\n')) {
    const trimmed = line.trim()
    if (!trimmed || trimmed.startsWith('#')) continue
    const eqIdx = trimmed.indexOf('=')
    if (eqIdx < 1) continue
    const key = trimmed.slice(0, eqIdx).trim()
    const val = trimmed.slice(eqIdx + 1).trim()
    pairs[key] = val
  }
  const found = new Set<string>()
  for (const key of Object.keys(pairs)) {
    for (const [prefix, serviceKey] of Object.entries(SERVICE_ENV_PREFIXES)) {
      if (key.startsWith(`${prefix}_`)) {
        found.add(serviceKey)
      }
    }
  }
  return { pairs, detectedServices: [...found] }
}
```

- [ ] **Step 2: Add `envText` state variable**

After the `syncingRef` declaration from Task 3, add:
```typescript
const [envText, setEnvText] = useState('')
```

- [ ] **Step 3: Add `applyEnvToForm` helper inside the component (before the return statement)**

```typescript
const applyEnvToForm = () => {
  const { pairs, detectedServices } = parseEnvText(envText)
  const detected = detectedServices[0]
  if (!detected) return
  const prefix = Object.entries(SERVICE_ENV_PREFIXES).find(([, key]) => key === detected)?.[0]
  if (!prefix) return
  setMode('custom')
  setTransport('http')
  setName(detected)
  const urlKey = `${prefix}_URL`
  if (pairs[urlKey]) setUrl(pairs[urlKey])
  setEnvDrawerOpen(false)
}
```

- [ ] **Step 4: Make `DialogContent` overflow-visible in `gateway-form-dialog.tsx`**

Change the `DialogContent` opening tag from:
```typescript
<DialogContent className="sm:max-w-[680px]">
```
to:
```typescript
<DialogContent className="relative overflow-visible sm:max-w-[540px]">
```

Note: width decreases from 680px to 540px per spec; the extra space was previously from content, now the narrower form is correct.

- [ ] **Step 5: Add the ENV drawer inside `DialogContent`, after `</Tabs>` but before the footer error block**

Locate the closing `</div>` of the scrollable body (`<div className="flex-1 min-h-0 overflow-y-auto -mx-6 px-6">`), then after that closing `</div>` and before the `{saveError && ...}` block, add:

```typescript
{/* ENV drawer */}
<div
  className={cn(
    'absolute top-0 bottom-0 bg-background border-l border-border rounded-r-lg overflow-hidden transition-[width] duration-[250ms] ease-[cubic-bezier(.4,0,.2,1)] flex flex-col',
    envDrawerOpen ? 'w-[300px]' : 'w-0',
  )}
  style={{ left: '100%' }}
  aria-hidden={!envDrawerOpen}
>
  <div className="flex flex-col gap-3 p-4 flex-1 overflow-y-auto">
    <p className="text-xs text-muted-foreground">
      Paste <code>KEY=VALUE</code> lines — Lab detects the service and can pre-fill the form.
    </p>
    <div className="relative">
      <textarea
        className="w-full min-h-[180px] rounded-md border border-border bg-background px-3 py-2 text-xs font-mono resize-none focus:outline-none focus:ring-2 focus:ring-ring"
        placeholder={'RADARR_URL=http://localhost:7878\nRADARR_API_KEY=abc123'}
        value={envText}
        onChange={(e) => setEnvText(e.target.value)}
      />
      {(() => {
        if (!envText.trim()) {
          return <span className="absolute top-2 right-2 text-[10px] text-muted-foreground">Waiting</span>
        }
        const { detectedServices } = parseEnvText(envText)
        if (detectedServices.length > 0) {
          return (
            <span className="absolute top-2 right-2 text-[10px] text-green-500">
              Valid · {detectedServices.length} service{detectedServices.length > 1 ? 's' : ''}
            </span>
          )
        }
        return <span className="absolute top-2 right-2 text-[10px] text-yellow-500">No known service</span>
      })()}
    </div>
    {envText.trim() && (() => {
      const { detectedServices } = parseEnvText(envText)
      if (detectedServices.length === 0) return null
      return (
        <div className="flex flex-wrap gap-1.5">
          {detectedServices.map((s) => (
            <span key={s} className="rounded-full bg-primary/10 border border-primary/30 px-2 py-0.5 text-xs text-primary">
              {s}
            </span>
          ))}
        </div>
      )
    })()}
  </div>
  <div className="flex gap-2 border-t border-border p-3">
    <button
      type="button"
      className="flex-1 rounded-md border border-border px-3 py-1.5 text-xs hover:bg-accent transition-colors"
      onClick={async () => {
        try {
          const text = await navigator.clipboard.readText()
          setEnvText(text)
        } catch {
          // clipboard access denied — user must paste manually
        }
      }}
    >
      Paste
    </button>
    <button
      type="button"
      className="flex-1 rounded-md bg-primary text-primary-foreground px-3 py-1.5 text-xs hover:bg-primary/90 transition-colors disabled:opacity-50"
      disabled={!parseEnvText(envText).detectedServices.length}
      onClick={applyEnvToForm}
    >
      Apply to form
    </button>
  </div>
</div>
```

- [ ] **Step 6: Add right-corner flattening when a drawer is open**

The `DialogContent` currently has `rounded-lg` from the base class. When a drawer is open the right corners should be `rounded-r-none`. Update the `DialogContent` className to:

```typescript
<DialogContent
  className={cn(
    'relative overflow-visible sm:max-w-[540px] transition-[border-radius] duration-[250ms]',
    (envDrawerOpen || jsonDrawerOpen) && 'rounded-r-none',
  )}
>
```

- [ ] **Step 7: TypeScript check**

```bash
cd apps/gateway-admin && npx tsc --noEmit 2>&1 | grep "error TS" | head -20
```

- [ ] **Step 8: Commit**

```bash
rtk git add apps/gateway-admin/components/gateway/gateway-form-dialog.tsx apps/gateway-admin/components/ui/dialog.tsx
rtk git commit -m "feat(gateway): add ENV slide-out drawer with service detection"
```

---

## Task 6: JSON drawer with two-way binding

**Files:**
- Modify: `apps/gateway-admin/components/gateway/gateway-form-dialog.tsx`

The JSON drawer shows a live-synced pretty-printed JSON editor. Form changes update JSON; valid JSON edits update the form. A `syncingRef` boolean prevents infinite loops. **Critical:** the guard reset must use `setTimeout(..., 0)` — a direct synchronous reset runs before React flushes the batched state updates, so the `useEffect` watching form fields sees `syncingRef.current = false` and immediately overwrites the drawer.

- [ ] **Step 1: Add a `buildJsonFromForm` helper (above the return statement)**

```typescript
const buildJsonFromForm = (): object | null => {
  const n = name.trim()
  if (!n) return null
  const cfg: Record<string, unknown> = {}
  if (transport === 'http') {
    const u = url.trim()
    if (u) cfg.url = u
  } else {
    const cmd = command.trim()
    if (cmd) cfg.command = cmd
    const a = args.trim()
    if (a) cfg.args = a.split(/\s+/).filter(Boolean)
  }
  return { [n]: cfg }
}
```

- [ ] **Step 2: Add `onFormChange` helper that pushes form state into the JSON textarea**

```typescript
const onFormChange = () => {
  if (syncingRef.current || !jsonDrawerOpen) return
  syncingRef.current = true
  const json = buildJsonFromForm()
  if (json) {
    setJsonText(JSON.stringify(json, null, 2))
    setJsonValid(true)
  } else {
    setJsonText('')
    setJsonValid(false)
  }
  // Defer reset so it runs AFTER React flushes batched state — otherwise the
  // useEffect watching [name, url, ...] fires with guard already false and loops.
  setTimeout(() => { syncingRef.current = false }, 0)
}
```

- [ ] **Step 3: Add a `useEffect` to call `onFormChange` when form fields change**

Add this `useEffect` inside the component (after the `onFormChange` definition, before the return statement):

```typescript
useEffect(() => {
  onFormChange()
  // eslint-disable-next-line react-hooks/exhaustive-deps
}, [name, url, command, args, transport, jsonDrawerOpen])
```

Do NOT add `onFormChange()` calls into individual `onChange` handlers — the effect guarantees it reads flushed state values.

- [ ] **Step 4: Add `parseJsonToForm` helper that reads JSON and populates form fields**

```typescript
const parseJsonToForm = (text: string) => {
  if (syncingRef.current) return
  try {
    const parsed = JSON.parse(text) as Record<string, unknown>
    const keys = Object.keys(parsed)
    if (keys.length !== 1) {
      setJsonValid(false)
      return
    }
    const gatewayName = keys[0]!
    const cfg = parsed[gatewayName] as Record<string, unknown>
    setJsonValid(true)
    syncingRef.current = true
    setName(gatewayName)
    if (typeof cfg.url === 'string') {
      setTransport('http')
      setUrl(cfg.url)
    } else if (typeof cfg.command === 'string') {
      setTransport('stdio')
      setCommand(cfg.command)
      if (Array.isArray(cfg.args)) {
        setArgs((cfg.args as string[]).join(' '))
      }
    }
    // Defer reset — same reason as onFormChange: the useEffect fires after React
    // flushes the setName/setUrl/setTransport calls; guard must still be true then.
    setTimeout(() => { syncingRef.current = false }, 0)
  } catch {
    setJsonValid(false)
  }
}
```

- [ ] **Step 5: Add JSON drawer markup after the ENV drawer block**

After the `{/* ENV drawer */}` block added in Task 5, add:

```typescript
{/* JSON drawer */}
<div
  className={cn(
    'absolute top-0 bottom-0 bg-background border-l border-border rounded-r-lg overflow-hidden transition-[width] duration-[250ms] ease-[cubic-bezier(.4,0,.2,1)] flex flex-col',
    jsonDrawerOpen ? 'w-[380px]' : 'w-0',
  )}
  style={{ left: '100%' }}
  aria-hidden={!jsonDrawerOpen}
>
  <div className="flex flex-col gap-3 p-4 flex-1 overflow-y-auto">
    <p className="text-xs text-muted-foreground">
      Live editor — changes here update the form, and form changes update this JSON automatically.
    </p>
    <div className="relative">
      <textarea
        className="w-full min-h-[240px] rounded-md border border-border bg-background px-3 py-2 text-xs font-mono resize-none focus:outline-none focus:ring-2 focus:ring-ring"
        placeholder={'{\n  "gateway-name": {\n    "url": "http://localhost:3001/mcp"\n  }\n}'}
        value={jsonText}
        onChange={(e) => {
          setJsonText(e.target.value)
          parseJsonToForm(e.target.value)
        }}
      />
      {(() => {
        if (!jsonText.trim()) {
          return <span className="absolute top-2 right-2 text-[10px] text-muted-foreground">Waiting</span>
        }
        if (jsonValid) {
          return <span className="absolute top-2 right-2 text-[10px] text-green-500">Valid</span>
        }
        return <span className="absolute top-2 right-2 text-[10px] text-destructive">Invalid JSON</span>
      })()}
    </div>
    {jsonValid && name && (
      <div className="flex flex-wrap gap-1.5">
        <span className="rounded-full bg-primary/10 border border-primary/30 px-2 py-0.5 text-xs text-primary">
          {name}
        </span>
        <span className="rounded-full bg-muted border border-border px-2 py-0.5 text-xs text-muted-foreground">
          {transport}
        </span>
      </div>
    )}
  </div>
  <div className="flex gap-2 border-t border-border p-3">
    <button
      type="button"
      className="flex-1 rounded-md border border-border px-3 py-1.5 text-xs hover:bg-accent transition-colors"
      onClick={async () => {
        try {
          const text = await navigator.clipboard.readText()
          setJsonText(text)
          parseJsonToForm(text)
        } catch {
          // clipboard access denied
        }
      }}
    >
      Paste
    </button>
  </div>
</div>
```

- [ ] **Step 6: TypeScript check**

```bash
cd apps/gateway-admin && npx tsc --noEmit 2>&1 | grep "error TS" | head -20
```

- [ ] **Step 7: Commit**

```bash
rtk git add apps/gateway-admin/components/gateway/gateway-form-dialog.tsx
rtk git commit -m "feat(gateway): add JSON drawer with two-way form sync"
```

---

## Task 7: Convert auth section to dropdown

**Files:**
- Modify: `apps/gateway-admin/components/gateway/gateway-form-dialog.tsx`

Spec section 3b: replace the auth `RadioGroup` with a shadcn `Select` dropdown trigger. The current three cards (no auth, bearer, OAuth) become dropdown items. The bearer and OAuth sub-sections (token input, OAuth connect button) remain below the trigger, shown conditionally as before. The auth section is hidden when stdio transport is selected — that behavior is unchanged.

- [ ] **Step 1: Add `Select` imports**

In the imports block at the top of `gateway-form-dialog.tsx`, add:

```typescript
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
```

Also add `ShieldOff` and `KeyRound` to the lucide-react import line (currently: `import { Loader2, Play, ShieldCheck, AlertCircle, CheckCircle2, ChevronRight } from 'lucide-react'`):

```typescript
import { Loader2, Play, ShieldCheck, AlertCircle, CheckCircle2, ChevronRight, ShieldOff, KeyRound } from 'lucide-react'
```

- [ ] **Step 2: Replace the auth RadioGroup with a Select dropdown**

Find this block (lines ~680–709):

```typescript
<RadioGroup value={authMode} onValueChange={(value) => setAuthMode(value as GatewayAuthMode)}>
  <label className="flex items-start gap-3 rounded-xl border p-4 cursor-pointer" htmlFor="auth-none">
    <RadioGroupItem value="none" id="auth-none" />
    <div className="space-y-1">
      <span className="font-medium text-sm">No auth</span>
      <p className="text-sm text-muted-foreground">No Authorization header sent.</p>
    </div>
  </label>
  <label className="flex items-start gap-3 rounded-xl border p-4 cursor-pointer" htmlFor="auth-bearer">
    <RadioGroupItem value="bearer" id="auth-bearer" />
    <div className="space-y-1">
      <span className="font-medium text-sm">Bearer token</span>
      <p className="text-sm text-muted-foreground">Static token sent as an Authorization header.</p>
    </div>
  </label>
  {transport === 'http' && (oauthProbed?.oauth_discovered || gateway?.config.oauth_enabled) && (
    <label className="flex items-start gap-3 rounded-xl border p-4 cursor-pointer" htmlFor="auth-oauth">
      <RadioGroupItem value="oauth" id="auth-oauth" />
      <div className="space-y-1">
        <span className="font-medium text-sm">
          OAuth (MCP)
          {oauthProbed?.oauth_discovered && (
            <Badge variant="secondary" className="ml-2 text-xs">Detected</Badge>
          )}
        </span>
        <p className="text-sm text-muted-foreground">OAuth 2.1 — for GitHub, Cloudflare, and other remote MCP servers.</p>
      </div>
    </label>
  )}
</RadioGroup>
```

Replace with:

```typescript
<Select value={authMode} onValueChange={(value) => setAuthMode(value as GatewayAuthMode)}>
  <SelectTrigger className="w-full">
    <SelectValue>
      <span className="flex items-center gap-2">
        {authMode === 'none' && <ShieldOff className="size-4 text-muted-foreground" />}
        {authMode === 'bearer' && <KeyRound className="size-4 text-muted-foreground" />}
        {authMode === 'oauth' && <ShieldCheck className="size-4 text-muted-foreground" />}
        {authMode === 'none' ? 'No auth' : authMode === 'bearer' ? 'Bearer token' : 'OAuth (MCP)'}
        {authMode === 'oauth' && oauthProbed?.oauth_discovered && (
          <Badge variant="secondary" className="ml-1 text-xs">Detected</Badge>
        )}
      </span>
    </SelectValue>
  </SelectTrigger>
  <SelectContent style={{ zIndex: 200 }}>
    <SelectItem value="none">
      <span className="flex items-center gap-2">
        <ShieldOff className="size-4 text-muted-foreground" />
        No auth
      </span>
    </SelectItem>
    <SelectItem value="bearer">
      <span className="flex items-center gap-2">
        <KeyRound className="size-4 text-muted-foreground" />
        Bearer token
      </span>
    </SelectItem>
    <SelectItem value="oauth">
      <span className="flex items-center gap-2">
        <ShieldCheck className="size-4 text-muted-foreground" />
        OAuth (MCP)
      </span>
    </SelectItem>
  </SelectContent>
</Select>
```

The `{authMode === 'oauth' && ...}` and `{authMode === 'bearer' && ...}` sub-sections that follow the RadioGroup (OAuth connect UI, bearer token input) remain exactly as they are — only the selector UI changes.

- [ ] **Step 3: Remove the `RadioGroup` / `RadioGroupItem` import if it is no longer used anywhere**

```bash
grep -n "RadioGroup\|RadioGroupItem" apps/gateway-admin/components/gateway/gateway-form-dialog.tsx
```

If the only remaining use is the transport RadioGroup (lines ~600–619), keep the import. If no uses remain, delete the import line.

- [ ] **Step 4: TypeScript check**

```bash
cd apps/gateway-admin && npx tsc --noEmit 2>&1 | grep "error TS" | head -20
```

- [ ] **Step 5: Commit**

```bash
rtk git add apps/gateway-admin/components/gateway/gateway-form-dialog.tsx
rtk git commit -m "feat(gateway): replace auth radio buttons with dropdown select"
```

---

## Task 8: Mobile responsive styles

**Files:**
- Modify: `apps/gateway-admin/components/gateway/gateway-form-dialog.tsx`

- [ ] **Step 1: Add mobile drawer override via Tailwind responsive classes**

The drawers use `style={{ left: '100%' }}` plus `w-[300px]`/`w-[380px]`. On mobile they should go full-width as a fixed overlay. Add a wrapper around the drawer `div` elements that adds mobile-override classes:

For the ENV drawer, change the `className` to:

```typescript
className={cn(
  // desktop: slide right of dialog
  'absolute top-0 bottom-0 bg-background border-l border-border rounded-r-lg overflow-hidden transition-[width] duration-[250ms] ease-[cubic-bezier(.4,0,.2,1)] flex flex-col',
  // mobile: full-screen fixed overlay
  'max-[600px]:fixed max-[600px]:inset-0 max-[600px]:rounded-none max-[600px]:border-l-0 max-[600px]:z-50',
  envDrawerOpen
    ? 'w-[300px] max-[600px]:w-full max-[600px]:h-full'
    : 'w-0',
)}
```

For the JSON drawer:

```typescript
className={cn(
  'absolute top-0 bottom-0 bg-background border-l border-border rounded-r-lg overflow-hidden transition-[width] duration-[250ms] ease-[cubic-bezier(.4,0,.2,1)] flex flex-col',
  'max-[600px]:fixed max-[600px]:inset-0 max-[600px]:rounded-none max-[600px]:border-l-0 max-[600px]:z-50',
  jsonDrawerOpen
    ? 'w-[380px] max-[600px]:w-full max-[600px]:h-full'
    : 'w-0',
)}
```

- [ ] **Step 2: Service grid mobile: 2 columns**

The service grid currently has `grid-cols-2 sm:grid-cols-3` (set in Task 2). Verify it is already there — no additional change needed.

- [ ] **Step 3: Transport radio cards: single column on mobile**

Find the transport RadioGroup:
```typescript
className="grid grid-cols-2 gap-3"
```

Change to:
```typescript
className="grid grid-cols-1 sm:grid-cols-2 gap-3"
```

- [ ] **Step 4: TypeScript check**

```bash
cd apps/gateway-admin && npx tsc --noEmit 2>&1 | grep "error TS" | head -20
```

- [ ] **Step 5: Commit**

```bash
rtk git add apps/gateway-admin/components/gateway/gateway-form-dialog.tsx
rtk git commit -m "feat(gateway): mobile-responsive drawer and grid breakpoints"
```

---

## Task 9: Update detail view to show proxy_prompts

**Files:**
- Modify: `apps/gateway-admin/components/gateway/gateway-detail-content.tsx`

- [ ] **Step 1: Add `promptExposureEnabled` computed value**

In `gateway-detail-content.tsx`, find line 290:
```typescript
const resourceExposureEnabled = gateway.config.proxy_resources ?? true
```

Add directly below it:
```typescript
const promptExposureEnabled = gateway.config.proxy_prompts ?? true
```

- [ ] **Step 2: Add `handleProxyPromptsToggle` handler**

Find the `handleProxyResourcesToggle` function (line ~352):
```typescript
const handleProxyResourcesToggle = async (enabled: boolean) => {
  try {
    await updateGateway(gateway.id, {
      config: {
        proxy_resources: enabled,
      },
    })
    toast.success(enabled ? 'Resource exposure enabled' : 'Resource exposure disabled')
  } catch (error) {
    toast.error(getErrorMessage(error, 'Failed to update resource exposure'))
  }
}
```

Add directly after it:
```typescript
const handleProxyPromptsToggle = async (enabled: boolean) => {
  try {
    await updateGateway(gateway.id, {
      config: {
        proxy_prompts: enabled,
      },
    })
    toast.success(enabled ? 'Prompt exposure enabled' : 'Prompt exposure disabled')
  } catch (error) {
    toast.error(getErrorMessage(error, 'Failed to update prompt exposure'))
  }
}
```

- [ ] **Step 3: Add "Expose prompts" toggle in the header**

Find the "Expose resources" toggle pill in the header (line ~389):
```typescript
{/* Expose resources toggle */}
<div className="inline-flex items-center gap-1.5 rounded-full border bg-background px-2.5 py-1">
  <span className="text-xs font-medium">Expose resources</span>
  <Switch
    aria-label="Expose resources"
    checked={resourceExposureEnabled}
    onCheckedChange={handleProxyResourcesToggle}
    className="scale-75"
  />
</div>
```

Add directly after it:
```typescript
{/* Expose prompts toggle */}
<div className="inline-flex items-center gap-1.5 rounded-full border bg-background px-2.5 py-1">
  <span className="text-xs font-medium">Expose prompts</span>
  <Switch
    aria-label="Expose prompts"
    checked={promptExposureEnabled}
    onCheckedChange={handleProxyPromptsToggle}
    className="scale-75"
  />
</div>
```

- [ ] **Step 4: TypeScript check**

```bash
cd apps/gateway-admin && npx tsc --noEmit 2>&1 | grep "error TS" | head -20
```

- [ ] **Step 5: Commit**

```bash
rtk git add apps/gateway-admin/components/gateway/gateway-detail-content.tsx
rtk git commit -m "feat(gateway): add proxy_prompts toggle to gateway detail header"
```

---

## Task 10: End-to-end verification

This task is not code — it is manual verification of the golden paths.

- [ ] **Step 1: Start the dev server**

```bash
cd apps/gateway-admin && npm run dev
```

- [ ] **Step 2: Verify Lab Service tab**

- Open the "Add Gateway" dialog
- Confirm 21 service cards appear in a 3-column grid (2-column on mobile)
- Confirm each card has a colored icon background with a white logo or SVG
- Confirm clicking a card selects it (highlighted border)
- Confirm ENV/JSON chips are NOT visible on the Lab Service tab

- [ ] **Step 3: Verify Custom tab — ENV drawer**

- Switch to Custom tab
- Confirm ENV chip is visible
- Click ENV — confirm drawer slides out from the right edge of the dialog, right corners of dialog flatten
- Paste `RADARR_URL=http://localhost:7878\nRADARR_API_KEY=testkey` into ENV textarea
- Confirm "Valid · 1 service" badge appears
- Confirm "radarr" pill appears
- Click "Apply to form" — confirm Name field populates with "radarr", URL populates with `http://localhost:7878`, drawer closes
- Click ENV chip again — confirm it closes (toggle behavior)

- [ ] **Step 4: Verify Custom tab — JSON drawer**

- Click JSON chip — confirm drawer slides out
- Confirm ENV drawer closes if it was open
- Type a gateway name in the form Name field — confirm JSON textarea updates live
- Type valid JSON in the textarea — confirm form fields update live
- Click JSON chip again — confirm it closes

- [ ] **Step 5: Verify auth dropdown**

- Confirm auth section shows a trigger button (not three radio cards)
- Confirm selecting "Bearer token" shows the token input below
- Confirm selecting "OAuth (MCP)" shows the connect button below
- Confirm auth section is hidden when stdio transport is selected

- [ ] **Step 6: Verify Proxy Prompts toggle**

- On Custom tab confirm "Proxy Prompts" toggle appears below "Proxy Resources"
- Toggle it off and save — confirm API payload includes `proxy_prompts: false`
- Open gateway detail — confirm "Expose prompts" toggle is visible in the header

- [ ] **Step 7: Run tests**

```bash
cd apps/gateway-admin && npm test 2>&1 | tail -20
```

- [ ] **Step 8: Final TypeScript clean build**

```bash
cd apps/gateway-admin && npx tsc --noEmit 2>&1 | grep "error TS" | wc -l
```

Expected: same count as before (no new errors).

---

## Implementation Notes

### Why `overflow-visible` on DialogContent

shadcn's `DialogContent` defaults to `overflow-hidden` (line 63 of `dialog.tsx`). The absolute-positioned drawers at `left: 100%` would be clipped. Passing `className="overflow-visible"` to `DialogContent` via `cn()`/`twMerge` correctly overrides this. The dialog is `position: fixed`, which makes it a positioned ancestor for the absolute drawers.

### Why `setTimeout` in the sync guard reset

React batches state updates — `setName()`, `setUrl()`, `setTransport()` called inside `parseJsonToForm` are flushed asynchronously. If `syncingRef.current = false` runs synchronously at the end of the function, the `useEffect` watching `[name, url, command, args, transport]` fires on the next render with the guard already cleared, sees changed state, and calls `onFormChange()` — which overwrites the JSON the user just typed. Deferring the reset via `setTimeout(..., 0)` ensures the guard is still `true` when that effect runs.

### Why inline SVGs for 7 services

Simple Icons CDN only covers services with open-source brand assets. Apprise, Arcane, ByteStash, Gotify, Linkding, Memos, and TEI are self-hosted services without registered Simple Icons entries. Inline SVGs avoid an external network dependency and load instantly.

### Why auth uses shadcn `Select` (not a custom dropdown)

shadcn `Select` uses a Radix UI portal that renders outside the dialog's DOM node, so it is never clipped by `overflow-hidden`. The `style={{ zIndex: 200 }}` on `SelectContent` ensures it appears above the dialog overlay. This is simpler and more accessible than a custom Popover-based dropdown.
