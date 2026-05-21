# CodeMirror Text Surface For Gateway Admin Implementation Plan

> Historical note: this plan captured the first implementation shape. The final implementation moved filesystem save/deploy into the Rust backend instead of `app/api`, and plugin details now render on `/marketplace/plugin?id=<pluginId>` rather than `plugin-detail-dialog.tsx`.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the current marketplace file viewer and gateway JSON overlay editor with a shared Aurora-aligned CodeMirror surface, backed by an app-managed workspace mirror with explicit save and explicit local deploy.

**Architecture:** Introduce a shared `TextSurface` UI layer for CodeMirror, a path-aware editor intelligence registry for language/validation behavior, a server-side workspace mirror for editable marketplace packages, and a local deploy adapter that syncs the saved workspace into the Claude Code target. Keep save and deploy separate, and keep filesystem/deploy concerns out of the editor component.

**Tech Stack:** Next.js 16, React 19, CodeMirror 6, TypeScript, existing `gateway-admin` component patterns, app-local server modules, Node filesystem APIs, existing test stack (`tsx --test`)

---

## File Structure

### Existing files to modify

- `apps/gateway-admin/package.json`
  - Add CodeMirror packages and update the dev script to bind on `0.0.0.0`.
- `apps/gateway-admin/README.md`
  - Document the `0.0.0.0` dev workflow and the new save/deploy editing behavior.
- `apps/gateway-admin/app/globals.css`
  - Add Aurora-compliant CodeMirror theme overrides and shared editor chrome styles.
- `apps/gateway-admin/components/marketplace/plugin-files-panel.tsx`
  - Replace Prism read-only rendering with the shared CodeMirror-based editor workflow.
- `apps/gateway-admin/components/marketplace/plugin-detail-dialog.tsx`
  - Wire save/deploy state and editable workspace-backed artifact loading.
- `apps/gateway-admin/components/gateway/gateway-form-dialog.tsx`
  - Replace the overlay JSON editor with the shared `TextSurface`.
- `apps/gateway-admin/lib/api/marketplace-client.ts`
  - Add client helpers for workspace read/save/deploy and artifact metadata normalization for editing.

### New UI/editor files to create

- `apps/gateway-admin/components/ui/text-surface.tsx`
  - Shared CodeMirror wrapper with read-only/edit modes, dirty state, language setup, search, folding, autocomplete, diagnostics, and Aurora chrome.
- `apps/gateway-admin/components/ui/text-surface-theme.ts`
  - CodeMirror theme + base extensions mapped to Aurora tokens.
- `apps/gateway-admin/components/ui/text-surface-toolbar.tsx`
  - Shared toolbar actions for save, deploy, copy, mode indicators, validation status.
- `apps/gateway-admin/components/ui/text-surface-status.tsx`
  - Shared status/diagnostic badge rendering.

### New editor intelligence files to create

- `apps/gateway-admin/lib/editor/types.ts`
  - Core document, validator, schema, and deploy result types.
- `apps/gateway-admin/lib/editor/language-registry.ts`
  - Path/file-type detection and lazy language extension loading.
- `apps/gateway-admin/lib/editor/diagnostics-registry.ts`
  - Registry that maps file path + language to validator/autocomplete providers.
- `apps/gateway-admin/lib/editor/frontmatter.ts`
  - Markdown frontmatter parsing and Claude Code frontmatter validation helpers.
- `apps/gateway-admin/lib/editor/json-schema.ts`
  - JSON schema selection and adapter helpers.
- `apps/gateway-admin/lib/editor/toml-schema.ts`
  - TOML validation/autocomplete hooks for known config shapes.

### New server/filesystem files to create

- `apps/gateway-admin/lib/server/marketplace-workspace.ts`
  - Workspace mirror creation, file enumeration, file reads/writes, dirty/saved semantics.
- `apps/gateway-admin/lib/server/claude-deploy.ts`
  - Local target resolution and explicit deploy from workspace mirror to Claude Code directories.
- `apps/gateway-admin/lib/server/marketplace-editor-service.ts`
  - Server-facing orchestration for load/save/deploy operations.

### New route files to create

- `apps/gateway-admin/app/api/marketplace/workspaces/[pluginId]/route.ts`
  - Load workspace tree and current file contents.
- `apps/gateway-admin/app/api/marketplace/workspaces/[pluginId]/files/route.ts`
  - Save edited files into the workspace mirror.
- `apps/gateway-admin/app/api/marketplace/workspaces/[pluginId]/deploy/route.ts`
  - Deploy saved workspace contents to the local Claude Code target.

### New test files to create

- `apps/gateway-admin/components/ui/text-surface.test.tsx`
- `apps/gateway-admin/lib/editor/language-registry.test.ts`
- `apps/gateway-admin/lib/editor/frontmatter.test.ts`
- `apps/gateway-admin/lib/editor/diagnostics-registry.test.ts`
- `apps/gateway-admin/lib/server/marketplace-workspace.test.ts`
- `apps/gateway-admin/lib/server/claude-deploy.test.ts`
- `apps/gateway-admin/lib/server/marketplace-editor-service.test.ts`
- `apps/gateway-admin/lib/api/marketplace-client-editing.test.ts`

## Task 1: Install editor dependencies and bind dev on `0.0.0.0`

**Files:**
- Modify: `apps/gateway-admin/package.json`
- Modify: `apps/gateway-admin/README.md`

- [ ] **Step 1: Add the failing package/test expectation to the plan notes**

Document the required packages before editing:

```text
@codemirror/state
@codemirror/view
@codemirror/language
@codemirror/search
@codemirror/commands
@codemirror/autocomplete
@codemirror/lint
@codemirror/lang-json
@codemirror/lang-markdown
@codemirror/lang-yaml
@codemirror/lang-javascript
@codemirror/lang-shell
@codemirror/lang-html (only if needed by current file set)
@codemirror/lang-css (only if needed by current file set)
@codemirror/lang-sql (skip unless current file set requires it)
@codemirror/lang-rust (skip unless current file set requires it)
@codemirror/lang-xml (skip unless current file set requires it)
@codemirror/lang-toml
```

- [ ] **Step 2: Update `package.json`**

Required edits:

```json
{
  "scripts": {
    "dev": "next dev -H 0.0.0.0"
  }
}
```

Add the CodeMirror dependencies needed for the scoped language/tooling set.

- [ ] **Step 3: Update `README.md`**

Add a local usage section that explicitly shows:

```bash
pnpm dev
# serves on 0.0.0.0
```

Also document that marketplace file editing now saves to an app-managed workspace mirror and deploys explicitly to the local Claude Code target.

- [ ] **Step 4: Verify dependency install metadata**

Run: `cd apps/gateway-admin && pnpm install`
Expected: lockfile updates cleanly with CodeMirror dependencies present

- [ ] **Step 5: Commit**

```bash
git add apps/gateway-admin/package.json apps/gateway-admin/README.md apps/gateway-admin/pnpm-lock.yaml
git commit -m "feat: add codemirror dependencies and 0.0.0.0 dev binding"
```

## Task 2: Build the shared Aurora `TextSurface`

**Files:**
- Create: `apps/gateway-admin/components/ui/text-surface.tsx`
- Create: `apps/gateway-admin/components/ui/text-surface-theme.ts`
- Create: `apps/gateway-admin/components/ui/text-surface-toolbar.tsx`
- Create: `apps/gateway-admin/components/ui/text-surface-status.tsx`
- Modify: `apps/gateway-admin/app/globals.css`
- Test: `apps/gateway-admin/components/ui/text-surface.test.tsx`

- [ ] **Step 1: Write the failing component tests**

Cover:

```tsx
it('renders read-only documents with line numbers and copy action', () => {})
it('renders editable documents and reports changes through onChange', () => {})
it('shows dirty state and validation status in the toolbar', () => {})
it('applies Aurora editor classes instead of Prism markup', () => {})
```

- [ ] **Step 2: Run the targeted test file to confirm failure**

Run: `cd apps/gateway-admin && pnpm test -- components/ui/text-surface.test.tsx`
Expected: FAIL because `text-surface.tsx` does not exist

- [ ] **Step 3: Implement the shared component and theme**

Minimum responsibilities:

```tsx
type TextSurfaceProps = {
  path: string
  value: string
  mode: 'view' | 'edit'
  language: 'json' | 'yaml' | 'markdown' | 'bash' | 'toml' | 'javascript' | 'typescript' | 'text'
  dirty?: boolean
  diagnostics?: EditorDiagnostic[]
  onChange?: (next: string) => void
  onSave?: () => void
  onDeploy?: () => void
  onCopy?: () => void
}
```

Implementation requirements:

- lazy extension assembly by language
- line numbers
- search keymap/panel
- fold gutter
- autocomplete
- lint/diagnostics support
- read-only mode
- toolbar slots for save/deploy/copy/status
- Aurora-mapped theme and surface styling

- [ ] **Step 4: Add global Aurora editor styles**

Add `.cm-editor`, `.cm-scroller`, `.cm-gutters`, `.cm-tooltip`, `.cm-panels`, `.cm-completionIcon`, `.cm-search`, and selection/cursor styling in `app/globals.css` using Aurora semantic tokens only.

- [ ] **Step 5: Run the component tests**

Run: `cd apps/gateway-admin && pnpm test -- components/ui/text-surface.test.tsx`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add apps/gateway-admin/components/ui/text-surface.tsx apps/gateway-admin/components/ui/text-surface-theme.ts apps/gateway-admin/components/ui/text-surface-toolbar.tsx apps/gateway-admin/components/ui/text-surface-status.tsx apps/gateway-admin/app/globals.css apps/gateway-admin/components/ui/text-surface.test.tsx
git commit -m "feat: add shared aurora codemirror text surface"
```

## Task 3: Add the editor intelligence registry

**Files:**
- Create: `apps/gateway-admin/lib/editor/types.ts`
- Create: `apps/gateway-admin/lib/editor/language-registry.ts`
- Create: `apps/gateway-admin/lib/editor/diagnostics-registry.ts`
- Create: `apps/gateway-admin/lib/editor/frontmatter.ts`
- Create: `apps/gateway-admin/lib/editor/json-schema.ts`
- Create: `apps/gateway-admin/lib/editor/toml-schema.ts`
- Test: `apps/gateway-admin/lib/editor/language-registry.test.ts`
- Test: `apps/gateway-admin/lib/editor/frontmatter.test.ts`
- Test: `apps/gateway-admin/lib/editor/diagnostics-registry.test.ts`

- [ ] **Step 1: Write the failing registry tests**

Cover:

```ts
it('maps marketplace file paths to the correct editor language', () => {})
it('treats .md agent/skill/command files as markdown plus frontmatter validation', () => {})
it('attaches JSON schema support to known JSON targets', () => {})
it('attaches TOML validation/autocomplete to known TOML targets', () => {})
it('returns no-op diagnostics for plain text files', () => {})
```

- [ ] **Step 2: Write the failing frontmatter tests**

Cover:

```ts
it('accepts valid claude agent frontmatter', () => {})
it('rejects missing required frontmatter fields', () => {})
it('ignores markdown files outside supported claude categories', () => {})
```

- [ ] **Step 3: Run the targeted tests to confirm failure**

Run: `cd apps/gateway-admin && pnpm test -- lib/editor/**/*.test.ts`
Expected: FAIL because the registry/frontmatter modules do not exist

- [ ] **Step 4: Implement the registry and validators**

Rules:

- `detectArtifactLang` logic moves into or delegates to `language-registry.ts`
- path-aware rules determine when markdown gets frontmatter validation
- JSON schemas are selected by path and file category
- TOML providers are selected by path and file category
- diagnostics output is normalized for `TextSurface`

Example API:

```ts
export function getEditorDocumentConfig(path: string): EditorDocumentConfig {
  return {
    language: 'markdown',
    validator: validateClaudeFrontmatter,
    autocomplete: undefined,
    schema: undefined,
  }
}
```

- [ ] **Step 5: Run the editor registry tests**

Run: `cd apps/gateway-admin && pnpm test -- lib/editor/language-registry.test.ts lib/editor/frontmatter.test.ts lib/editor/diagnostics-registry.test.ts`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add apps/gateway-admin/lib/editor/types.ts apps/gateway-admin/lib/editor/language-registry.ts apps/gateway-admin/lib/editor/diagnostics-registry.ts apps/gateway-admin/lib/editor/frontmatter.ts apps/gateway-admin/lib/editor/json-schema.ts apps/gateway-admin/lib/editor/toml-schema.ts apps/gateway-admin/lib/editor/*.test.ts
git commit -m "feat: add editor language and diagnostics registry"
```

## Task 4: Implement the workspace mirror and local deploy adapter

**Files:**
- Create: `apps/gateway-admin/lib/server/marketplace-workspace.ts`
- Create: `apps/gateway-admin/lib/server/claude-deploy.ts`
- Create: `apps/gateway-admin/lib/server/marketplace-editor-service.ts`
- Test: `apps/gateway-admin/lib/server/marketplace-workspace.test.ts`
- Test: `apps/gateway-admin/lib/server/claude-deploy.test.ts`
- Test: `apps/gateway-admin/lib/server/marketplace-editor-service.test.ts`

- [ ] **Step 1: Write the failing workspace/deploy tests**

Cover workspace mirror:

```ts
it('creates a plugin workspace mirror preserving relative paths', () => {})
it('writes file saves atomically into the workspace mirror', () => {})
it('reads the current saved workspace state for a plugin', () => {})
```

Cover deploy:

```ts
it('deploys saved workspace files into the local claude target', () => {})
it('refuses deploy when the workspace reports unsaved changes', () => {})
it('returns a structured deploy summary', () => {})
```

- [ ] **Step 2: Run the targeted server tests to confirm failure**

Run: `cd apps/gateway-admin && pnpm test -- lib/server/marketplace-workspace.test.ts lib/server/claude-deploy.test.ts lib/server/marketplace-editor-service.test.ts`
Expected: FAIL because the new modules do not exist

- [ ] **Step 3: Implement the workspace mirror**

Requirements:

- app-managed root directory for editable marketplace packages
- mirror exact relative paths from artifacts
- file read/write APIs
- save metadata/dirty-state support
- atomic writes

Example service contract:

```ts
export async function saveWorkspaceFile(input: {
  pluginId: string
  path: string
  content: string
}): Promise<{ savedAt: string }>
```

- [ ] **Step 4: Implement local deploy**

Requirements:

- resolve local Claude Code target directories explicitly
- copy/sync saved workspace files only
- report changed/skipped/failed files
- keep deploy separate from save

- [ ] **Step 5: Implement orchestration service**

`marketplace-editor-service.ts` should coordinate:

- initial workspace hydration from artifacts if absent
- file load
- file save
- deploy preconditions
- deploy execution

- [ ] **Step 6: Run the server tests**

Run: `cd apps/gateway-admin && pnpm test -- lib/server/marketplace-workspace.test.ts lib/server/claude-deploy.test.ts lib/server/marketplace-editor-service.test.ts`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add apps/gateway-admin/lib/server/marketplace-workspace.ts apps/gateway-admin/lib/server/claude-deploy.ts apps/gateway-admin/lib/server/marketplace-editor-service.ts apps/gateway-admin/lib/server/*.test.ts
git commit -m "feat: add marketplace workspace mirror and local deploy"
```

## Task 5: Expose workspace save/load/deploy over app routes and client API

**Files:**
- Create: `apps/gateway-admin/app/api/marketplace/workspaces/[pluginId]/route.ts`
- Create: `apps/gateway-admin/app/api/marketplace/workspaces/[pluginId]/files/route.ts`
- Create: `apps/gateway-admin/app/api/marketplace/workspaces/[pluginId]/deploy/route.ts`
- Modify: `apps/gateway-admin/lib/api/marketplace-client.ts`
- Test: `apps/gateway-admin/lib/api/marketplace-client-editing.test.ts`

- [ ] **Step 1: Write the failing client API tests**

Cover:

```ts
it('loads workspace files for a plugin', () => {})
it('saves an edited file through the workspace API', () => {})
it('deploys a plugin workspace through the deploy API', () => {})
it('surfaces structured save and deploy errors', () => {})
```

- [ ] **Step 2: Run the targeted tests to confirm failure**

Run: `cd apps/gateway-admin && pnpm test -- lib/api/marketplace-client-editing.test.ts`
Expected: FAIL because the new API helpers do not exist

- [ ] **Step 3: Implement the route handlers**

Route responsibilities:

- `GET /api/marketplace/workspaces/[pluginId]`
  - return workspace tree, saved file contents, dirty metadata if needed
- `PUT /api/marketplace/workspaces/[pluginId]/files`
  - save a file into the workspace mirror
- `POST /api/marketplace/workspaces/[pluginId]/deploy`
  - deploy the saved workspace into the local Claude Code target

- [ ] **Step 4: Extend `marketplace-client.ts`**

Add helpers such as:

```ts
export async function getPluginWorkspace(pluginId: string): Promise<PluginWorkspace> {}
export async function savePluginWorkspaceFile(input: SavePluginFileInput): Promise<SavePluginFileResult> {}
export async function deployPluginWorkspace(pluginId: string): Promise<DeployPluginWorkspaceResult> {}
```

- [ ] **Step 5: Run the client API tests**

Run: `cd apps/gateway-admin && pnpm test -- lib/api/marketplace-client-editing.test.ts`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add apps/gateway-admin/app/api/marketplace/workspaces apps/gateway-admin/lib/api/marketplace-client.ts apps/gateway-admin/lib/api/marketplace-client-editing.test.ts
git commit -m "feat: add marketplace workspace save and deploy api"
```

## Task 6: Migrate the marketplace file panel to the shared editor workflow

**Files:**
- Modify: `apps/gateway-admin/components/marketplace/plugin-files-panel.tsx`
- Modify: `apps/gateway-admin/components/marketplace/plugin-detail-dialog.tsx`
- Modify: `apps/gateway-admin/lib/api/marketplace-client.ts`
- Test: `apps/gateway-admin/components/marketplace/plugin-files-panel.test.tsx` (create if missing)

- [ ] **Step 1: Write the failing marketplace editor tests**

Cover:

```tsx
it('loads plugin files into the shared TextSurface', () => {})
it('marks files dirty after edits', () => {})
it('saves the active file to the workspace mirror', () => {})
it('blocks deploy when there are unsaved changes', () => {})
it('deploys the saved plugin workspace on explicit action', () => {})
```

- [ ] **Step 2: Run the targeted tests to confirm failure**

Run: `cd apps/gateway-admin && pnpm test -- components/marketplace/plugin-files-panel.test.tsx`
Expected: FAIL because the editor workflow does not exist yet

- [ ] **Step 3: Replace Prism with `TextSurface`**

Required changes:

- remove inline Prism highlighting/rendering logic
- load editable workspace-backed file contents
- use the language registry instead of local language detection
- surface validation status and dirty state
- wire explicit save and deploy actions

- [ ] **Step 4: Update the dialog/container state**

`plugin-detail-dialog.tsx` should:

- fetch workspace-aware file state instead of raw artifact-only display state
- hold package-level save/deploy status
- pass save/deploy callbacks to the editor

- [ ] **Step 5: Run the marketplace editor tests**

Run: `cd apps/gateway-admin && pnpm test -- components/marketplace/plugin-files-panel.test.tsx`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add apps/gateway-admin/components/marketplace/plugin-files-panel.tsx apps/gateway-admin/components/marketplace/plugin-detail-dialog.tsx apps/gateway-admin/components/marketplace/plugin-files-panel.test.tsx apps/gateway-admin/lib/api/marketplace-client.ts
git commit -m "feat: migrate marketplace files to shared codemirror editor"
```

## Task 7: Migrate the gateway JSON drawer to the shared editor

**Files:**
- Modify: `apps/gateway-admin/components/gateway/gateway-form-dialog.tsx`
- Test: `apps/gateway-admin/components/gateway/gateway-form-dialog.test.tsx` (extend or create targeted coverage)

- [ ] **Step 1: Write the failing gateway JSON editor tests**

Cover:

```tsx
it('renders the JSON drawer using TextSurface', () => {})
it('shows JSON diagnostics inline', () => {})
it('keeps form sync with editor changes', () => {})
it('removes the legacy overlay textarea implementation', () => {})
```

- [ ] **Step 2: Run the targeted tests to confirm failure**

Run: `cd apps/gateway-admin && pnpm test -- components/gateway/gateway-form-dialog.test.tsx`
Expected: FAIL for the new TextSurface-driven expectations

- [ ] **Step 3: Replace the legacy drawer implementation**

Requirements:

- remove the `pre` + transparent `textarea` overlay
- mount `TextSurface` in edit mode
- feed JSON diagnostics from the registry
- preserve existing form synchronization behavior
- keep the compact drawer ergonomics aligned with Aurora

- [ ] **Step 4: Run the gateway editor tests**

Run: `cd apps/gateway-admin && pnpm test -- components/gateway/gateway-form-dialog.test.tsx`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add apps/gateway-admin/components/gateway/gateway-form-dialog.tsx apps/gateway-admin/components/gateway/gateway-form-dialog.test.tsx
git commit -m "feat: migrate gateway json drawer to shared codemirror editor"
```

## Task 8: Final focused-rollout integration coverage and docs polish

**Files:**
- Modify: `apps/gateway-admin/README.md`
- Modify: `docs/superpowers/specs/2026-04-22-codemirror-gateway-admin-design.md` (only if implementation drift needs documenting)
- Test: existing targeted tests plus a focused integration selection

- [ ] **Step 1: Add or extend integration tests for the complete flow**

Required coverage:

- marketplace edit -> save -> deploy
- gateway JSON edit with diagnostics
- TOML validation on a known config file target
- frontmatter validation on a supported markdown file target

- [ ] **Step 2: Run the focused integration/test suite**

Run:

```bash
cd apps/gateway-admin
pnpm test -- lib/editor/**/*.test.ts lib/server/*.test.ts lib/api/marketplace-client-editing.test.ts components/ui/text-surface.test.tsx components/marketplace/plugin-files-panel.test.tsx components/gateway/gateway-form-dialog.test.tsx
```

Expected: PASS

- [ ] **Step 3: Run the app lint/test commands used by the app**

Run:

```bash
cd apps/gateway-admin
pnpm lint
pnpm test
```

Expected:

- `pnpm lint` passes
- `pnpm test` passes

- [ ] **Step 4: Update docs if implementation-specific behavior changed**

If the final workspace/deploy behavior or local target assumptions changed from the spec, update the spec or README for accuracy.

- [ ] **Step 5: Commit**

```bash
git add apps/gateway-admin/README.md docs/superpowers/specs/2026-04-22-codemirror-gateway-admin-design.md
git commit -m "docs: finalize codemirror editor workflow documentation"
```

## Implementation Notes

- Keep marketplace editing and deploy orchestration in app-specific files. Do not let `TextSurface` know about filesystem paths or Claude Code targets.
- Treat save and deploy as different state machines in the UI.
- Prefer lazy language extension loading to avoid front-loading every language bundle.
- Use the existing design-system contract as the authority for every visible editor state.
- Avoid over-designing remote target support now; preserve boundaries only.
- Future `.claude-plugin/marketplace.json` and `.claude-plugin/plugin.json` validation should plug into `diagnostics-registry.ts`, not spawn new editor surfaces.

## Definition Of Done

- CodeMirror is the single text surface for the focused rollout.
- Marketplace files are editable, saveable to the workspace mirror, and deployable to the local Claude Code target.
- Gateway JSON editing uses the shared editor.
- JSON and TOML diagnostics/autocomplete are present.
- Claude frontmatter validation is present for supported file categories.
- The UI stays Aurora-compliant.
- Local development runs on `0.0.0.0`.
