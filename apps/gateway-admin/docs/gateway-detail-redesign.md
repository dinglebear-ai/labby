# Gateway Detail View — Redesign Spec

## Summary

Reorganize the gateway detail page to surface useful information immediately, reduce scrolling, and eliminate redundant/noisy UI. The current layout buries the tabs below a large card that wastes prime viewing area on client JSON and a redundant probe-result banner. This spec consolidates the layout so the tabs are the primary content area and everything secondary moves into a header strip or its own tab.

---

## Changes

### 1. List View — Status Label Removal (already done)

**File:** `components/gateway/gateway-table.tsx`

- Remove the `<span>` that renders `statusTone.label` ("HEALTHY", "DISCONNECTED") next to the dot in both mobile and desktop rows.
- Keep the dot. Move the label to `aria-label` + `title` for accessibility only.

---

### 2. List View — Warnings Badge Count (already done)

**File:** `components/gateway/warnings-pill.tsx`

- Change `{warnings.length} issue{…}` to just `{warnings.length}`.
- The icon + count is sufficient; "issue/issues" is noise.

---

### 3. List View — Lab Virtual Server Endpoint Text (already done)

**File:** `lib/api/gateway-mobile.ts`

- Replace `"<name> in-process service"` with `labby mcp --services <name>` so the row shows the actual connect command.

---

### 4. Detail View — Move Tabs Directly Under the Name/Endpoint Block

**Current layout:**
```
[Card]
  Status dot + Name
  Endpoint
  SurfaceRatio chips (Tools 29/29 | Resources 0/0 | Prompts 0/0)
  Toggles strip
  2-col grid: Client JSON | Probe result card
[Tabs below the card]
  Tools | Resources | Prompts | Warnings
```

**New layout:**
```
[Header block]
  Status dot + Name
  Endpoint
  [Tabs — right after the endpoint, counts merged in]
    Tools (N) | Resources (N) | Prompts (N) | Warnings (N) | Config
[Tab content — full width, no extra card wrapper]
```

**Details:**
- Remove the separate `SurfaceRatio` chip row — the tab trigger counts replace them. The tab trigger itself reads e.g. `Tools 1` (not a separate badge; inline number).
- Remove the 2-column grid card (Client JSON + probe banner) from the header block entirely.
- The Tabs component moves inside the header block, positioned below the name/endpoint lines.

---

### 5. Detail View — Config Tab (replaces inline Client JSON)

**Add a new `Config` tab.** Content:

- Client JSON block (moved from the current inline display).
- Not the default active tab; default remains `tools`.
- No changes to `buildGatewayClientConfig` — just a display relocation.

---

### 6. Detail View — Remove "Most Recent Probe Result" Banner

**File:** `components/gateway/gateway-detail-content.tsx` lines 511–543

Remove the entire conditional block:
```tsx
{gateway.status.last_error ? (
  <div className="rounded-xl border border-warning/20 …">
    <AlertTriangle … />
    Most recent probe result
    {gateway.status.last_error}
  </div>
) : (
  <div className="rounded-xl border border-success/20 …">
    Gateway reachable
    …
  </div>
)}
```

Also remove the adjacent "All discovered tools are currently exposed…" summary paragraph (lines 533–543).

> **Historical design record:** The examples below use `demo-upstream` as a
> placeholder and do not describe the current runtime inventory.

**Where this info moves:**
- Warning details are accessible via the **Warnings tab** (already exists).
- The warning badge in the header (`1 warning`) should show a tooltip on hover with the first warning message, so the user never has to click in to get a quick summary.
- Tooltip should be added to the existing `<Badge>` at line 410–413.

---

### 7. Detail View — Move Surface Toggles to the AppHeader Strip

**Current location:** A `flex-wrap` pill row inside the main card (lines 459–499):
```
• Offline  Expose resources [toggle]  • CLI [toggle]  • API [toggle]  • MCP [toggle]  • WEBUI [toggle]
```

**New location:** Right-hand side of the `AppHeader`, replacing the current action buttons row (or alongside it). The header strip currently shows `GATEWAY | 1 warning | Updated …` on the right. The toggles should live in this same horizontal strip between the breadcrumbs and the timestamp.

**Layout of the new header strip:**
```
[Breadcrumbs: Gateways > demo-upstream]       [• Offline] [Expose resources ⚙] [• CLI ○] [• API ○] [• MCP ●] [• WEBUI ○]   [Updated …]
```

- Each toggle remains a pill with the status dot, surface label, and switch.
- The `Expose resources` toggle stays in this strip as well.
- The `Test`, `Reload`, `Edit`, `Disable/Remove` action buttons move into a compact `…` overflow menu or stay as-is on a secondary row if space is tight — do not remove them.

---

### 8. Detail View — Prompts: Show Expandable Content

**Current state:** Prompts tab lists prompt names and argument badges but no way to see the full prompt template or detailed argument definitions.

**New behavior:**
- Each prompt card should be an expandable accordion or a click-to-expand row.
- Expanded state shows:
  - `description` (already displayed, promote it)
  - Each argument: `name`, `description`, `required` flag, rendered as a small definition list
  - If the upstream exposes a `template` or `content` field on the prompt, render it as a `<pre><code>` block
- The collapsed state shows only the prompt name + argument count badge (as today).

---

### 9. Detail View — Resources: Fix Nested/Recursive URIs

**Historical example:** Resources for the `demo-upstream` (STDIO) gateway showed URIs like:
```
lab://upstream/demo-upstream/lab://upstream/demo-upstream/lab://upstream/demo-upstream/lab://catalog
```
Each row appends the full upstream prefix again, making URIs exponentially longer. This is a **backend bug** in resource URI construction — the upstream pool or gateway manager is prepending the prefix on URIs that already carry it.

**Root cause to investigate:**
- `crates/lab/src/dispatch/upstream/pool.rs` — resource URI rewrite logic when proxying resources from an upstream MCP server
- Check whether the rewrite is applied unconditionally vs. only when the URI doesn't already contain the prefix
- The fix: before prepending `lab://upstream/<name>/`, check `if uri.starts_with("lab://upstream/")` and skip rewriting if true

**Frontend (display only — does not fix the underlying bug):**
- In the Resources tab, truncate displayed URIs in the list with `text-ellipsis overflow-hidden` and show the full URI in a `title` tooltip so the card doesn't overflow.
- Add a "Copy URI" button per row.

---

### 10. Detail View — Tools vs. Actions Distinction for In-Process Services

**Historical example:** For a lab in-process service (represented here as `demo-upstream`), the "Tools" count showed 29 because the in-process service exposed 29 actions mapped as MCP tools (one per service action). The custom gateway (`demo-upstream` as a STDIO upstream) showed one tool that dispatched through an `action` parameter.

**User concern:** These are architecturally different:
- **Custom STDIO gateway**: 1 MCP tool (`demo-upstream`) with N sub-actions dispatched via `action` parameter — tool count = 1, action count = N
- **In-process service**: N tools, each mapping directly to a service action — tool count = N

**Display change:**
- For `source === 'in_process'` gateways, rename the "Tools" tab label to "Actions" and show the action count.
- The `SurfaceRatio` component used in the list view should also show "Actions" label for lab services instead of "Tools".
- For custom gateways using lab's `action + params` dispatch pattern: if `discovered_tool_count === 1` and the single tool has sub-actions discoverable via `help`, consider showing "1 tool / N actions". This requires the upstream pool to surface sub-action counts, which is a backend enhancement — defer to a follow-up.

---

### 11. Server Name Collision Detection

**Historical example:** Two gateways could share the same name/id (e.g., two `demo-upstream` entries). This caused ambiguous routing, duplicate display rows, and potential data corruption on update/delete.

**Changes needed:**

**Backend (`crates/lab/src/dispatch/gateway/manager.rs` or `api/router.rs`):**
- On `POST /gateways` (add gateway), check whether a gateway with the same name already exists.
- Return `409 Conflict` with a structured error: `{ "kind": "conflict", "message": "A gateway named 'demo-upstream' already exists.", "existing_id": "demo-upstream" }`.
- Same check on in-process service enable if a custom gateway with the same name exists.

**Frontend (`components/gateway/gateway-form-dialog.tsx`):**
- On submit, catch `409` responses and display an inline error: `"A gateway named '<name>' already exists. Choose a different name or remove the existing gateway first."`
- Do not dismiss the dialog on 409 — let the user fix the name in-place.

---

## Non-goals (questions, not changes)

- **Why does the in-process service not expose resources/prompts?** This is a probe behavior difference: the in-process service health check hits `GET /healthz` directly and does not run a full MCP `initialize` + `resources/list` + `prompts/list` probe. The STDIO upstream does run the full MCP handshake, so it discovers resources and prompts. Not a UI issue — architectural note only.

- **Action count for non-lab MCP servers using `action + params` dispatch**: Requires backend support to surface sub-action counts from the upstream's `help` response. Deferred.
