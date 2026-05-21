# Gateway Dialog Redesign

**Date:** 2026-04-20
**Status:** Approved
**Scope:** `apps/gateway-admin/components/gateway/gateway-form-dialog.tsx` and related types/components

## Overview

Redesign the Add Gateway dialog in `gateway-admin` to improve usability for both Lab Service and Custom gateway flows. The validated design was iterated interactively via a browser mockup at `.superpowers/brainstorm/2396884-1776718812/content/gateway-dialog-interactive.html`.

---

## 1. Dialog Shell

**Width:** 540px (fixed). No height constraint — content determines height; body has a `min-height` so the dialog never feels cramped.

**Two tabs** rendered as a pill-style segmented control:
- **Lab Service** — configure a built-in lab service as a gateway
- **Custom** — manually configure any upstream MCP server

**Header** shows the dialog title and subtitle. On the Custom tab only, the header right-side shows two chip buttons: **ENV** and **JSON**. Both are hidden on the Lab Service tab (`visibility: hidden` so the header height stays constant).

**Footer** always shows Cancel + a primary action button. The primary label changes by tab:
- Lab Service tab → "Configure Service" (disabled until a service is selected)
- Custom tab → "Add Gateway" (disabled until Name + URL/Command are filled)

---

## 2. Lab Service Tab

### Service grid

- 3-column grid of service cards, `max-height` capped to show exactly 3 rows (~320px) with `overflow-y: auto` and a styled scrollbar for the remaining 12+ services.
- Each card shows: brand-colored icon box (36×36px with rounded corners), service name, category badge, and a 2-line description clamp.
- **Brand-colored icon backgrounds:** every service icon box uses the service's brand color (e.g., Radarr gold, Sonarr sky-blue, Plex orange) with the white variant of the logo on top. For services not in Simple Icons (Apprise, Arcane, ByteStash, Gotify, Linkding, Memos, TEI), a white inline SVG renders on the brand background. No external favicon APIs.
- **Logo sources:** Simple Icons CDN (`cdn.simpleicons.org/{slug}/ffffff`) for 14 services; inline SVG for 7 services.
- Clicking a card selects it (highlighted border + accent background tint).

### After selecting a service

Below the grid (separated by a divider), show:
- Dynamic env var fields for the selected service — each field uses `type="password"` for secret vars and `type="text"` for non-secret vars, with example placeholder and description text.
- **Enable gateway** toggle card — on by default; exposes the service as a visible gateway immediately on save.

### 21 supported services

Apprise, Arcane, ByteStash, Gotify, Linkding, Memos, OpenAI, Overseerr, Paperless, Plex, Prowlarr, qBittorrent, Qdrant, Radarr, SABnzbd, Sonarr, Tailscale, Tautulli, TEI, UniFi, Unraid.

---

## 3. Custom Tab

The dialog body always shows the standard form. Two optional slide-out drawers (ENV, JSON) extend from the right side and are toggled via header chips.

### 3a. Form fields

| Field | Condition | Notes |
|---|---|---|
| Name | Always | Lowercase alphanumeric + hyphens; "Add Gateway" button is disabled until non-empty |
| Transport | Always | Two radio cards: HTTP / stdio |
| URL | HTTP transport only | Placeholder `http://localhost:3001/mcp` |
| Command + Args | stdio transport only | Two-column grid; Args is space-separated |
| Authentication | HTTP transport only | Hidden when stdio selected |
| Proxy Resources | Always | Toggle, default on |
| Proxy Prompts | Always | Toggle, default on |

### 3b. Authentication dropdown

Replaces radio buttons. A trigger button shows the current selection with icon + label. Opens a dropdown menu (not clipped — the dialog shell has no `overflow: hidden`; the menu uses `z-index: 200`).

Three options:
- **No auth** — no Authorization header sent
- **Bearer token** — static token field appears below trigger when selected
- **OAuth (MCP)** — OAuth 2.1 flow for remote MCP servers

Auth section is hidden entirely when stdio transport is selected.

### 3c. ENV drawer

Activated by the **ENV** chip in the header. Slides out 300px from the right edge of the dialog. The dialog's right border-radius morphs to `0` to connect seamlessly with the drawer. Opening ENV closes the JSON drawer if open.

**Contents:**
- Hint: "Paste `KEY=VALUE` lines for a known service — Lab detects it and pre-fills the form."
- Live `KEY=VALUE` textarea with badge showing parse state (Waiting / Valid N services / Invalid format / No known service)
- Detected service pills shown below editor
- Footer: Paste (reads clipboard) + Apply to form buttons
- Apply populates the Custom form: Name = detected service key (e.g. `radarr`), Transport = `http`, URL = value of `{SERVICE}_URL` env var if present. Closes the drawer on apply.

**Service detection:** match env var prefixes (`RADARR_`, `SONARR_`, etc.) against the 21 known services.

### 3d. JSON drawer

Activated by the **JSON** chip in the header. Slides out 380px from the right. Opening JSON closes the ENV drawer if open.

**Live two-way binding:** the JSON drawer and the Custom form stay in sync.
- Editing the form → JSON textarea updates immediately (pretty-printed, 2-space indent)
- Editing valid JSON in the drawer → form fields update immediately
- A `syncing` guard flag prevents infinite loops

**Contents:**
- Hint: "Live editor — changes here update the form, and form changes update this JSON automatically."
- JSON textarea with badge (Waiting / Valid · transport detected / Invalid JSON)
- Detected name + transport pills
- Footer: Paste only (no Apply button — sync is live)

**JSON format:**
```json
{
  "gateway-name": {
    "url": "http://localhost:3001/mcp"
  }
}
```
or for stdio:
```json
{
  "gateway-name": {
    "command": "npx",
    "args": ["-y", "some-mcp-server"]
  }
}
```
Exactly one top-level key required. Any other shape is marked invalid.

---

## 4. Drawer Mechanics

Both drawers share the same CSS pattern:

```css
/* Drawer positioned outside dialog right edge */
.env-drawer, .json-drawer {
  position: absolute;
  top: 0; left: 100%; bottom: 0;
  width: 0; overflow: hidden;
  transition: width .25s cubic-bezier(.4, 0, .2, 1);
  border-radius: 0 12px 12px 0;
}
.env-drawer.open  { width: 300px; }
.json-drawer.open { width: 380px; }

/* Dialog right corners flatten when a drawer is open */
.dialog.drawer-open { border-radius: 12px 0 0 12px; }
```

Rules:
- Only one drawer can be open at a time — opening one closes the other
- Both drawers close when switching tabs
- Clicking the active chip closes its drawer (toggle behavior)
- On mobile (`max-width: 600px`), drawers use `position: fixed` and cover the full screen

---

## 5. Mobile Behavior

At ≤600px:
- Dialog fills the full viewport (no horizontal padding, `border-radius: 0`)
- Drawers use `position: fixed`, `width: 100%` — full-screen overlay
- Transport radio grid collapses to single column
- Proxy toggle row stacks vertically
- Auth select expands to full width
- Service grid collapses to 2 columns

---

## 6. Types to Add/Update

### `lib/types/gateway.ts`

Add `proxy_prompts: boolean` to `GatewayConfig` (currently only `proxy_resources` exists).

### `GatewayAuthMode`

Existing type `'none' | 'bearer' | 'oauth'` — no change needed.

### New local state in `gateway-form-dialog.tsx`

```typescript
// Drawer state
envDrawerOpen: boolean   // default false
jsonDrawerOpen: boolean  // default false

// Proxy
proxyPrompts: boolean    // default true  (proxy_resources already exists)

// Live JSON sync
jsonText: string         // pretty-printed JSON mirror of form state
jsonValid: boolean
```

---

## 7. Files to Change

| File | Change |
|---|---|
| `components/gateway/gateway-form-dialog.tsx` | Primary: all UX changes above |
| `components/gateway/gateway-list-content.tsx` | Pass `proxy_prompts` in submit payload |
| `components/gateway/gateway-detail-content.tsx` | Display `proxy_prompts` value |
| `lib/types/gateway.ts` | Add `proxy_prompts` to `GatewayConfig` |
| `components/ui/` | No new primitives needed — use existing Dialog, Switch, Input |

The `lab-service-picker.tsx` component will be replaced by the inline service grid in the redesigned dialog — it is no longer needed as a separate component.

---

## 8. Out of Scope

- Backend changes to the gateway API (the `proxy_prompts` field must already be accepted by the server, or added there separately)
- Any change to the gateway list view beyond the detail panel
- The upstream OAuth section (separate feature)
- Any change to the Rust backend
