# Chat AI Component Upgrade Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade `/chat` by selectively porting the highest-value vendored AI components into the Aurora/Labby chat surface without violating the design-system contract.

**Architecture:** Keep `/chat` transcript-first and Aurora-native. Reuse vendored AI components only where they improve artifact rendering or interaction density, and wrap/adapt them behind existing chat-specific boundaries instead of letting a second component language leak into product code. Preserve the current controller split and the canonical ACP event flow.

**Tech Stack:** Next.js 16, React 19, TypeScript, existing `apps/gateway-admin/components/chat/*`, vendored `apps/gateway-admin/components/ai/*`, Aurora tokens and primitives from `components/ui/*` and `components/aurora/*`.

---

## File Structure

### Existing files to modify

- `apps/gateway-admin/components/chat/message-bubble.tsx`
  - Assistant-turn container; current host for chain-of-thought, reasoning, and inline tool-call flow.
- `apps/gateway-admin/components/chat/tool-call-display.tsx`
  - Current Aurora-native tool/action renderer; primary integration point for richer artifact components.
- `apps/gateway-admin/components/chat/chat-input.tsx`
  - Current product-native composer; should remain the source of truth unless a vendored primitive yields a narrowly scoped improvement.
- `apps/gateway-admin/components/chat/types.ts`
  - Chat-facing state types; extend only if new artifact subtypes need explicit UI state.
- `apps/gateway-admin/lib/chat/session-events.ts`
  - Derives transcript/tool-call state from ACP events; may need richer artifact metadata extraction.
- `apps/gateway-admin/components/chat/tool-call-presentation.tsx`
  - Existing presentation adapter for tool calls; likely best place to route new artifact categories.
- `apps/gateway-admin/components/chat/activity-timeline.tsx`
  - Reference-only unless explicitly revived; do not reintroduce a split transcript/activity workspace by accident.
- `apps/gateway-admin/components/chat/message-thread.tsx`
  - May need minor composition changes if new artifact blocks change message layout.
- `apps/gateway-admin/components/chat/session-sidebar.tsx`
  - Only touch if `/chat` upgrade introduces shared pattern changes relevant to drawers/secondary panels.
- `apps/gateway-admin/components/chat/settings-panel.tsx`
  - Only touch if shared chat component upgrades create a design-system alignment issue here.

### Vendored AI components to evaluate for direct use

- `apps/gateway-admin/components/ai/code-block.tsx`
- `apps/gateway-admin/components/ai/terminal.tsx`
- `apps/gateway-admin/components/ai/file-tree.tsx`
- `apps/gateway-admin/components/ai/confirmation.tsx`
- `apps/gateway-admin/components/ai/sources.tsx`
- `apps/gateway-admin/components/ai/web-preview.tsx`

### Tests to modify or add

- `apps/gateway-admin/lib/acp/normalize.test.ts`
- `apps/gateway-admin/lib/chat/session-events.test.ts`
- `apps/gateway-admin/lib/chat/use-session-events.test.ts`
- `apps/gateway-admin/lib/browser/chat-shell.browser.test.ts`
- Add targeted component tests near the chat component being upgraded if the current suite lacks direct coverage.

### Docs/design references to consult while implementing

- `docs/design/design-system-contract.md`
- `docs/acp/design.md`

## Scope Guardrails

- Do not replace the custom `ToolCallDisplay` with the generic vendored `Tool` card.
- Do not replace the custom `ChatInput` with the vendored `prompt-input`.
- Do not reintroduce a split transcript/activity workspace; keep activity inline inside assistant turns.
- Do not port vendored components wholesale. Port or mine only the pieces that materially improve the current `/chat` experience.
- Do not add raw colors, shadcn-generic product tokens, arbitrary radii, or hand-rolled eyebrow typography.

## Implementation Order

1. Upgrade artifact rendering inside the existing tool/action flow.
2. Upgrade permission/approval rendering if ACP events support it cleanly.
3. Upgrade source/web/file preview rendering where the current transcript metadata already exposes enough structure.
4. Only after those land, consider any composer or settings-panel refinements.

### Task 1: Add `code-block` as the canonical expanded output renderer

**Files:**
- Modify: `apps/gateway-admin/components/chat/tool-call-display.tsx`
- Modify: `apps/gateway-admin/components/chat/tool-call-presentation.tsx`
- Test: `apps/gateway-admin/lib/chat/session-events.test.ts`

- [ ] **Step 1: Write the failing test**

Add a test case that derives a tool call with structured output and expects the UI-facing tool-call state to preserve enough metadata for formatted code/JSON rendering rather than raw string-only fallback.

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm test -- session-events.test.ts`
Expected: FAIL because the current flow does not distinguish formatted code-block rendering behavior.

- [ ] **Step 3: Write minimal implementation**

Import and use `@/components/ai/code-block` inside the expanded Input/Output sections of `ToolCallDisplay` so JSON/text payloads render through the shared code renderer while preserving Aurora shell styling.

Implementation notes:
- Keep the surrounding Aurora panel chrome in `ToolCallDisplay`.
- Use `CodeBlock` only for the actual payload body.
- Preserve current string vs object behavior.

- [ ] **Step 4: Run targeted test**

Run: `pnpm test -- session-events.test.ts`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add apps/gateway-admin/components/chat/tool-call-display.tsx apps/gateway-admin/components/chat/tool-call-presentation.tsx apps/gateway-admin/lib/chat/session-events.test.ts
git commit -m "feat(chat): use shared code block for tool payloads"
```

### Task 2: Add `terminal` rendering for command-heavy tool artifacts

**Files:**
- Modify: `apps/gateway-admin/components/chat/tool-call-display.tsx`
- Modify: `apps/gateway-admin/components/chat/tool-call-presentation.tsx`
- Test: `apps/gateway-admin/lib/chat/session-events.test.ts`
- Test: `apps/gateway-admin/lib/browser/chat-shell.browser.test.ts`

- [ ] **Step 1: Write the failing test**

Add a transcript test and/or browser fixture that includes command/stdout-style artifact data and expects the resulting tool call to render as terminal-like output rather than a plain paragraph block.

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm test -- session-events.test.ts chat-shell.browser.test.ts`
Expected: FAIL because command artifacts currently render as generic summary/snippet blocks.

- [ ] **Step 3: Write minimal implementation**

Integrate `@/components/ai/terminal` into the command/stdout-specific branch of `ToolCallDisplay`.

Implementation notes:
- Gate this on command/log-like artifact types only.
- Keep Aurora tone; do not let the vendored component introduce a separate styling language.
- Preserve the connected inline tool timeline around it.

- [ ] **Step 4: Run targeted tests**

Run: `pnpm test -- session-events.test.ts chat-shell.browser.test.ts`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add apps/gateway-admin/components/chat/tool-call-display.tsx apps/gateway-admin/components/chat/tool-call-presentation.tsx apps/gateway-admin/lib/chat/session-events.test.ts apps/gateway-admin/lib/browser/chat-shell.browser.test.ts
git commit -m "feat(chat): render command artifacts with terminal view"
```

### Task 3: Add `file-tree` / file-preview rendering for structured file artifacts

**Files:**
- Modify: `apps/gateway-admin/components/chat/tool-call-display.tsx`
- Modify: `apps/gateway-admin/components/chat/tool-call-presentation.tsx`
- Modify: `apps/gateway-admin/lib/chat/session-events.ts`
- Test: `apps/gateway-admin/lib/chat/session-events.test.ts`

- [ ] **Step 1: Write the failing test**

Add a tool-call derivation test that feeds file-tree/file-preview style artifact data and expects normalized transcript state to carry enough structure for dedicated rendering.

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm test -- session-events.test.ts`
Expected: FAIL because file artifacts are currently reduced to generic snippet/path handling.

- [ ] **Step 3: Write minimal implementation**

Use `@/components/ai/file-tree` for actual tree-like artifacts and preserve the current Aurora preview shell for simpler path/snippet cases.

Implementation notes:
- Do not replace every file-related artifact with the vendored component.
- Only use it where the data is truly hierarchical.
- Keep flat/single-file previews in the lighter existing renderer when that reads better.

- [ ] **Step 4: Run targeted test**

Run: `pnpm test -- session-events.test.ts`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add apps/gateway-admin/components/chat/tool-call-display.tsx apps/gateway-admin/components/chat/tool-call-presentation.tsx apps/gateway-admin/lib/chat/session-events.ts apps/gateway-admin/lib/chat/session-events.test.ts
git commit -m "feat(chat): add structured file artifact rendering"
```

### Task 4: Add `confirmation` rendering for ACP permission events

**Files:**
- Modify: `apps/gateway-admin/components/chat/message-bubble.tsx`
- Modify: `apps/gateway-admin/components/chat/tool-call-display.tsx`
- Modify: `apps/gateway-admin/lib/chat/session-events.ts`
- Test: `apps/gateway-admin/lib/chat/session-events.test.ts`
- Test: `apps/gateway-admin/lib/acp/normalize.test.ts`

- [ ] **Step 1: Write the failing test**

Add tests that feed `permission.requested` and `permission.resolved` ACP events and expect the assistant turn to surface a dedicated approval/permission presentation instead of treating them as generic activity only.

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm test -- session-events.test.ts normalize.test.ts`
Expected: FAIL because permission events are not yet rendered through a dedicated confirmation UI.

- [ ] **Step 3: Write minimal implementation**

Integrate `@/components/ai/confirmation` where permission events appear in the assistant turn.

Implementation notes:
- Keep permissions inline with the turn, not in a separate pane.
- Use the vendored component only if it can be styled with Aurora tokens without forcing a second visual language.
- If the vendored surface is too generic, mine its structure and keep the custom shell.

- [ ] **Step 4: Run targeted tests**

Run: `pnpm test -- session-events.test.ts normalize.test.ts`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add apps/gateway-admin/components/chat/message-bubble.tsx apps/gateway-admin/components/chat/tool-call-display.tsx apps/gateway-admin/lib/chat/session-events.ts apps/gateway-admin/lib/chat/session-events.test.ts apps/gateway-admin/lib/acp/normalize.test.ts
git commit -m "feat(chat): render permission events inline"
```

### Task 5: Add `sources` and `web-preview` for search/browser artifacts

**Files:**
- Modify: `apps/gateway-admin/components/chat/tool-call-display.tsx`
- Modify: `apps/gateway-admin/components/chat/tool-call-presentation.tsx`
- Modify: `apps/gateway-admin/lib/chat/session-events.ts`
- Test: `apps/gateway-admin/lib/chat/session-events.test.ts`
- Test: `apps/gateway-admin/lib/browser/chat-shell.browser.test.ts`

- [ ] **Step 1: Write the failing test**

Add transcript/browser tests for source/citation and URL-preview artifacts emitted from tool calls.

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm test -- session-events.test.ts chat-shell.browser.test.ts`
Expected: FAIL because links currently render as simple chips and previews are not first-class.

- [ ] **Step 3: Write minimal implementation**

Use `@/components/ai/sources` and `@/components/ai/web-preview` where the artifact shape actually supports them.

Implementation notes:
- Keep them subordinate to the inline tool timeline.
- Do not promote them to full-width dominant cards unless the data requires it.
- Preserve keyboard accessibility and token-only Aurora styling.

- [ ] **Step 4: Run targeted tests**

Run: `pnpm test -- session-events.test.ts chat-shell.browser.test.ts`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add apps/gateway-admin/components/chat/tool-call-display.tsx apps/gateway-admin/components/chat/tool-call-presentation.tsx apps/gateway-admin/lib/chat/session-events.ts apps/gateway-admin/lib/chat/session-events.test.ts apps/gateway-admin/lib/browser/chat-shell.browser.test.ts
git commit -m "feat(chat): add source and web preview artifacts"
```

### Task 6: Design-system cleanup in the current `/chat` surface

**Files:**
- Modify: `apps/gateway-admin/components/chat/message-thread.tsx`
- Modify: `apps/gateway-admin/components/chat/settings-panel.tsx`
- Modify: `apps/gateway-admin/components/chat/chat-input.tsx`
- Test: existing relevant browser/component tests

- [ ] **Step 1: Write the failing test or checklist assertion**

Capture the current design drift to fix:
- hand-rolled eyebrow styling in the empty state
- any missing `aurora-scrollbar` on explicit overflow containers
- any local typography/radius drift introduced while porting new AI components

- [ ] **Step 2: Run focused checks**

Run: targeted component/browser tests covering chat render surfaces
Expected: identify or reproduce drift before fixing it

- [ ] **Step 3: Write minimal implementation**

Clean up `/chat` to match the design contract:
- replace hand-rolled eyebrow styling with `AURORA_MUTED_LABEL`
- ensure new overflow containers include `aurora-scrollbar`
- keep Display tokens and Aurora panel tiers intact

- [ ] **Step 4: Run targeted checks**

Run: targeted browser/component tests for affected files
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add apps/gateway-admin/components/chat/message-thread.tsx apps/gateway-admin/components/chat/settings-panel.tsx apps/gateway-admin/components/chat/chat-input.tsx
git commit -m "chore(chat): align upgraded chat surface with design contract"
```

### Task 7: Explicitly reject low-value vendored components for `/chat`

**Files:**
- Modify: `docs/acp/design.md`
- Modify: `docs/design/design-system-contract.md` only if a new shared pattern is introduced
- Modify: optional local chat docs if they exist

- [ ] **Step 1: Write the failing docs diff goal**

Document that `/chat` intentionally keeps custom implementations for tool cards and prompt input instead of adopting vendored `tool` and `prompt-input` directly.

- [ ] **Step 2: Update docs**

Record the architectural choice and the allowed vendored-component usage pattern.

- [ ] **Step 3: Review for DRY/YAGNI**

Ensure docs describe only the shipped decisions and not future speculative ports.

- [ ] **Step 4: Commit**

```bash
git add docs/acp/design.md docs/design/design-system-contract.md
git commit -m "docs(chat): record selective AI component adoption strategy"
```

## Testing Strategy

Minimum test bar for the full plan:

- `pnpm test -- normalize.test.ts session-events.test.ts use-session-events.test.ts chat-shell.browser.test.ts`
- Add direct component tests if a ported AI component introduces logic that is hard to verify through transcript/browser tests alone.
- If a port affects shared browser types or imports broadly, run the app-level TypeScript/lint target already used by `apps/gateway-admin`.

## Acceptance Criteria

- `/chat` remains transcript-first and inline; no split activity workspace is introduced.
- Ported AI components are visually absorbed into Aurora/Labby styling and do not import a second design language.
- The highest-value artifact classes render better than today: structured code, terminal logs, file trees, permissions, sources, and previews.
- `tool.tsx` and `prompt-input.tsx` remain unused in `/chat`.
- The design-system contract remains satisfied: Aurora tokens only, approved typography usage, restrained states, and compact operator density.
