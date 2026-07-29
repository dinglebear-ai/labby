# Setup + Settings — Feature Design Spec

**Status:** Locked  
**Bead:** lab-bg3e (epic) → lab-bg3e.1–5  
**Mockups:** `/dev/setup`, `/dev/settings`  
**Date:** 2026-04-25  
**Primary contract:** [Design System Contract](../../design/design-system-contract.md)
**Component development process:** [Component Development](../../design/component-development.md)

---

## Problem and Scope

Labby has no first-run configuration experience. New users must manually edit `~/.labby/.env` and `~/.labby/config.toml`. The Setup wizard replaces that with a guided 7-step flow. The Settings page replaces the current gateway-focused stub with a full configuration rail for all 23 services and surfaces.

Both surfaces are **re-runnable** — Setup detects existing config and pre-populates all fields from the live environment.

---

## Architecture

### Data Sources

| Source | What it provides | Endpoint |
|--------|-----------------|----------|
| `~/.labby/.env` | Service URLs, API keys, tokens (masked) | `GET /dev/api/systeminfo` → `env` map |
| `~/.labby/config.toml` | Bind host, master URL, node controller | `GET /dev/api/systeminfo` → `local_host`, `controller`, `master_url` |
| `/v1/nodes` | Live node connectivity (requires token) | `GET /v1/nodes` |

`/dev/api/systeminfo` is unauthenticated and reads env vars from the running process (dotenvy loads `.env` at startup). Secrets are masked as `***` — the UI treats `***` as "value already set; leave blank to keep."

### Write Path

All writes go through `setup.draft.set` + `setup.draft.commit` to `~/.labby/.env.draft`. Nothing touches `~/.labby/.env` until Finalize & Commit. A timestamped backup is created before every write.

---

## Setup Wizard (`/setup`)

### Flow

7 steps in two phases:

**Phase 1 — Gating (no sidebar, full-width, linear)**

| Step | Title | Gate condition |
|------|-------|---------------|
| 1 | Welcome | None |
| 2 | Core Config | None |
| 3 | PreFlight | All 5 system checks pass |

**Phase 2 — Configuration (sidebar slides in, free-nav)**

| Step | Title |
|------|-------|
| 4 | Services |
| 5 | Surfaces |
| 6 | PreFlight 2 |
| 7 | Finalize |

Phase 2 unlocks when PreFlight 1 passes. The sidebar lists all 21 services by category plus Core, Surfaces, PreFlight 2, and Finalize. Users navigate freely — Finalize & Commit is always reachable from the pinned sidebar button.

### Stepper

- Full 7-step stepper visible from step 1 (users see the complete journey upfront)
- Phase 1: stepper below topbar with step labels
- Phase 2: stepper collapses into compact topbar strip (dots only, no labels)
- Mobile (<600px): labels hidden, active step name shown as subtitle below dots

### Re-run mode

On load, the wizard calls `/dev/api/systeminfo`. If env values exist:
- Re-run banner shown on step 1
- All fields pre-populated (secrets as `***`)
- Nodes fleet restored from `node.controller` + live `/v1/nodes`
- Auth mode set from `LAB_AUTH_MODE` or presence of Google credentials

### Step 2: Core Config

Three panels:

**Server**
- Bind Address (`LAB_MCP_HTTP_HOST`, default `127.0.0.1`) — with warning if `0.0.0.0` without auth
- Port (`LAB_MCP_HTTP_PORT`, default `8765`) — no spinner arrows, shared by Web UI + API + MCP
- Log Level (`LAB_LOG`) — Aurora dropdown, tracing filter syntax
- Log Format (`LAB_LOG_FORMAT`) — Aurora dropdown, `text` or `json`

**Authentication**

Radio toggle: Bearer Token vs OAuth (Google)

*Bearer:* `LAB_MCP_HTTP_TOKEN` — password field with eye toggle + ⟳ Generate button (crypto.getRandomValues, 32-byte URL-safe)

*OAuth:* three fields (all pre-populated from env):
- Public URL (`LAB_PUBLIC_URL`) — required, used for metadata + callback
- Google Client ID (`LAB_GOOGLE_CLIENT_ID`)
- Google Client Secret (`LAB_GOOGLE_CLIENT_SECRET`, masked)

Switches to OAuth mode automatically if `LAB_AUTH_MODE=oauth` or Google credentials present.

**Nodes**

- This device always at index 0 as `THIS DEVICE + MASTER`, locked checkbox, master radio pre-selected
- Hostname resolved from `/dev/api/systeminfo` → `controller` field (same as `node.controller` in config.toml)
- Additional nodes from `/v1/nodes` with **live** connected status
- "Scan ~/.ssh/config" button appends SSH hosts from `deploy.config.list`
- Manual add field for SSH aliases
- Master URL (`deploy.defaults.master_url`) auto-populated when master selected; editable
- Node connectivity: ● Connected (live from `/v1/nodes`), ○ Offline

### Step 4: Services

21 services across 10 categories. Each service has:
- Icon: selfhst CDN SVG → PNG fallback → brand-color letter div
- Icon background: `rgba(brandColor, 0.13)` with `rgba(brandColor, 0.30)` border
- Fields driven by `svc.fields[]` array (envKey, type, placeholder, hint, optional)
- Field types: `url`, `password` (eye toggle, `***` placeholder when set), `text`
- Test Connection: simulated in mockup; real implementation calls `doctor.service_probe`
- Values pre-populated from `/dev/api/systeminfo → env`

**Service catalog with exact env vars:**

| Service | URL key | Auth key | Auth type |
|---------|---------|----------|-----------|
| ExampleMovies | `EXAMPLEMOVIES_URL` | `EXAMPLEMOVIES_API_KEY` | X-Api-Key header |
| ExampleSeries | `EXAMPLESERIES_URL` | `EXAMPLESERIES_API_KEY` | X-Api-Key header |
| ExampleIndexer | `EXAMPLEINDEXER_URL` | `EXAMPLEINDEXER_API_KEY` | X-Api-Key header |
| ExampleRequests | `EXAMPLEREQUESTS_URL` | `EXAMPLEREQUESTS_API_KEY` | X-Api-Key header |
| ExampleMetrics | `EXAMPLEMETRICS_URL` | `EXAMPLEMETRICS_API_KEY` | ?apikey= query param |
| ExampleMedia | `EXAMPLEMEDIA_URL` | `EXAMPLEMEDIA_TOKEN` | X-ExampleMedia-Token header |
| ExampleUsenet | `EXAMPLEUSENET_URL` | `EXAMPLEUSENET_API_KEY` | ?apikey= query param |
| exampledownload | `EXAMPLEDOWNLOAD_URL` | `EXAMPLEDOWNLOAD_USERNAME` + `EXAMPLEDOWNLOAD_PASSWORD` | Session cookie |
| Unraid | `UNRAID_URL` | `UNRAID_API_KEY` | X-API-Key header |
| UniFi | `UNIFI_URL` | `UNIFI_API_KEY` | X-API-KEY header |
| Tailscale | `TAILSCALE_URL` | `TAILSCALE_API_KEY` | Bearer token |
| Arcane | `ARCANE_URL` | `ARCANE_API_KEY` (optional) | Optional |
| Linkding | `LINKDING_URL` | `LINKDING_TOKEN` | Authorization: Token |
| Memos | `MEMOS_URL` | `MEMOS_TOKEN` | Authorization: Bearer |
| Paperless | `PAPERLESS_URL` | `PAPERLESS_TOKEN` | Authorization: Token |
| Bytestash | `BYTESTASH_URL` | `BYTESTASH_TOKEN` | Authorization: Bearer |
| Gotify | `GOTIFY_URL` | `GOTIFY_TOKEN` | X-Gotify-Key header |
| Apprise | `APPRISE_URL` | — | No auth |
| OpenAI | `OPENAI_URL` | `OPENAI_API_KEY` | Authorization: Bearer |
| Qdrant | `QDRANT_URL` | `QDRANT_API_KEY` (optional) | api-key header |
| TEI | `TEI_URL` | `TEI_TOKEN` (optional) | Authorization: Bearer |

### Step 5: Surfaces

Six surfaces with checkboxes derived from live env:

| Surface | Env var | Default | Mutual exclusion |
|---------|---------|---------|-----------------|
| Web UI | `LAB_WEB_ASSETS_DIR` | ✓ | — |
| HTTP API | `LAB_MCP_HTTP_TOKEN` | ✓ | — |
| MCP (HTTP) | `LAB_MCP_TRANSPORT=http` | ✓ | Mutex with stdio |
| MCP (stdio) | `LAB_MCP_TRANSPORT=stdio` | ✗ | Mutex with HTTP |
| TUI | — | ✗ | — |
| OAuth (Google) | `LAB_AUTH_MODE=oauth` | from env | Shows sub-fields |

OAuth sub-fields (Public URL, Client ID, Client Secret) pre-populated from env. Panel shown automatically if OAuth is detected.

### Step 3: PreFlight 1

Real HTTP checks via `fetch()` against the running server. Mirrors `scripts/check-oauth.sh` §2–5, §8:

| # | Check | Script section |
|---|-------|----------------|
| 1 | Server reachable — `GET /health → 200` | §2 |
| 2 | Protected endpoints reject unauthenticated — `GET /v1/doctor → 401` | §3 |
| 3 | Bearer token authenticates — `GET /v1/doctor/actions with Bearer → 200` | §4 |
| 4 | MCP endpoint bearer-only — `/mcp` with fake session cookie → 401 | §5 |
| 5 | Dev preview accessible — `POST /dev/api/marketplace → 200/400` (not 401) | §8 |

All 5 must pass to unlock phase 2. Checks run sequentially with real fetch() calls; spinner per row animates to ✓/✗.

### Step 6: PreFlight 2

Full validation from `scripts/check-oauth.sh` §2–8, §10 plus service-specific probes:

| # | Check | Script section |
|---|-------|----------------|
| 1 | Server reachable | §2 |
| 2 | Protected endpoints require auth | §3 |
| 3 | Bearer token authenticates | §4 |
| 4 | MCP endpoint bearer-only | §5 |
| 5 | OAuth discovery metadata (if OAuth mode) — `/.well-known/oauth-authorization-server` | §6 |
| 6 | JWKS keys endpoint (if OAuth mode) | §6 |
| 7 | WWW-Authenticate header on 401s (if OAuth mode) | §7 |
| 8 | Upstream OAuth callback route is public | §10 |
| 9 | Dev preview accessible | §8 |
| 10–14 | Per-service URL reachability + credential probe (first 5 configured services) | — |
| Last | Draft integrity check | — |

OAuth-specific checks (5–8) only run when `LAB_AUTH_MODE=oauth` or Google credentials present. All-green enables Finalize & Commit.

### Step 7: Finalize

Confirmation modal: warns write is irreversible, shows backup path (`~/.labby/.env.bak.<ts>`). On confirm → success screen with 🎉, "Go to Overview" + "Open Settings" buttons.

---

## Settings Page (`/settings`)

### Layout

Three-column: 180px app nav sidebar + 160px settings rail + content area.

**App nav sidebar** — same full navigation as the rest of the app (Overview, Gateways, Marketplace, Setup, Settings, Activity, Logs). Settings is the active item. Logo: 30×30 network node SVG.

**Settings rail sections:**
- CONFIG: Core, Services
- SYSTEM: Doctor, Extract
- V2 STUBS: Surfaces (v2), Features (v2), Advanced (v2)

### Core Panel

Same fields as Setup step 2 with optimistic save (each Save writes immediately to `~/.labby/.env`, no draft). Auth mode badge (Bearer / OAuth) shown read-only with link to reconfigure.

### Services Panel

Table: SERVICE | STATUS | ENABLED | ACTIONS

Inline expand on Configure. Fields use same `svc.fields[]` definitions as Setup. Save button per service. "Leave blank to keep current" placeholder for masked secrets.

### Doctor Panel

Summary cards: Services Checked, Passing, Warnings, Failing.
Per-service audit list with icon, name, version string, auth status, pass/fail badge.

### Extract Panel

- Scan for Credentials button → animated → results table (Key / Value / Source / Include checkbox)
- Apply to ~/.labby/.env button (shows diff first, then applies with backup)
- Preview Diff button

### v2 Stub Panels

Surfaces, Features, Advanced each show a "Coming in v2" placeholder with description.

---

## Design System Compliance

### Typography
- Page/section titles: `Manrope` 700–800, Display 1 (34px) or Display 2 (19px)
- Body copy: `Inter` 400 14px
- Controls, labels: `Inter` 500–600 13px
- Eyebrow labels: `Inter` 700 10px, `0.14em` tracking, uppercase
- Monospace (code): Cascadia Code / Fira Code

### Colors
All colors via Aurora CSS variables. Raw hex only in service brand backgrounds (sanctioned exception per contract). No `rgba()` with raw values outside `:root` composites.

### Components
- **Inputs:** Aurora control surface bg, `--aurora-border-strong`, `--aurora-focus-ring` on focus
- **Dropdowns:** Custom Aurora dropdown (`createAuroraSelect`) — no native `<select>` in visible UI
- **Number inputs:** Spinner arrows removed (`-webkit-appearance: none`)
- **Buttons:** Primary (accent fill), Secondary (surface + border), Ghost (text only), Destructive (error family)
- **Toggles:** `accent-color: var(--accent)`
- **Icons:** selfhst CDN SVG → PNG → letter fallback; brand-color bg `rgba(color, 0.13)` + border `rgba(color, 0.30)`

### Motion
- Sidebar slide-in: `cubic-bezier(0.4, 0, 0.2, 1)` 350ms
- Step transitions: `fadeIn` 220ms ease
- PreFlight checks: sequential 700ms per check
- Finalize button: `opacity` transition 200ms

---

## Deviations from Earlier Spec (lab-bg3e)

| Decision | Spec | Final |
|----------|------|-------|
| Surfaces/Features in wizard | Steps 4+5 in linear flow | Step 5 as free-nav sidebar item after PreFlight |
| Wizard step count | 8 steps | 7 steps (Surfaces/Features merged, no separate Config step) |
| Settings auth | Separate step | Part of Core panel |
| Nodes section | In Services step | In Core Config step 2 |
| Re-run behavior | Opens /settings | Opens /setup with pre-populated fields |

All deviations approved during brainstorm and mockup iteration sessions.

---

## Known Limitations (v1)

- **Light mode:** Mockups are dark-only. React implementation will inherit light mode via `next-themes` and Aurora `.light` remaps.
- **Real test connections:** Mockup uses random simulation. Real implementation calls `doctor.service_probe`.
- **Real write path:** Mockup has no backend. Real implementation writes through `setup.draft.set` + `setup.draft.commit`.
- **Tailnet field:** `TAILSCALE_TAILNET` read from env but Tailscale uses `TAILSCALE_API_KEY` not `TAILSCALE_TOKEN` — env key confirmed from live `.env`.
- **generateStaticParams:** Settings `/settings/services/[service]` requires `generateStaticParams()` for `output:'export'`.

---

## Implementation Notes

See beads lab-bg3e.1–5 for phase-by-phase implementation plan. Phase 2 (doctor dispatch) is complete. Phase 1 (UiSchema/PluginMeta extensions) is in progress.

The mockup HTML files in `~/.superpowers/brainstorm/content/` are Tier 1 renders per `docs/design/component-development.md §5`. They are discarded when the real React pages land at `app/(admin)/setup/` and `app/(admin)/settings/`.
