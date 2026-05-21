# CodeMirror Text Surface For Gateway Admin

**Date:** 2026-04-22  
**Status:** Proposed  
**Scope:** `apps/gateway-admin`

## Summary

`gateway-admin` should adopt a single Aurora-aligned CodeMirror-based text surface for both read-only viewing and editing. The first rollout replaces the existing Prism marketplace file viewer and the custom JSON overlay editor with a shared component, adds filesystem-backed saves into an app-managed workspace mirror, and adds explicit deploy into the local Claude Code instance.

The rollout is intentionally scoped to:

- marketplace file viewing and editing
- the gateway JSON drawer
- other clearly existing text/code detail panels in the app that fit the same shared component
- explicit `Save` to an app-managed workspace mirror
- explicit `Deploy` from that workspace mirror into the local Claude Code target

Out of scope for this phase:

- remote device deployment
- a user-owned git repo workflow
- broad migration of every `textarea` in the app
- full schema support for every language

## Goals

- Use one shared CodeMirror-based component for view and edit text surfaces.
- Keep the editor visually compliant with the Labby Aurora design system contract in dark and light themes.
- Make marketplace files editable in the UI.
- Save edited files to an app-managed on-disk workspace mirror.
- Deploy saved workspace contents into the local Claude Code installation through an explicit user action.
- Provide strong JSON and TOML diagnostics/autocomplete.
- Validate Claude Code frontmatter for targeted agent, skill, and command files when possible.
- Preserve an architecture that can later support remote deploy targets without rewriting the editor layer.

## Non-Goals

- Implementing remote targets in this phase
- Implementing a full repo management workflow in this phase
- Binding editor behavior directly to deploy logic
- Shipping CodeMirror default styles or accepting design drift from third-party UI chrome

## Existing State

Current text/code surfaces are inconsistent:

- marketplace files use a custom Prism-based read-only viewer in `components/marketplace/plugin-files-panel.tsx`
- the gateway JSON drawer uses a `pre` plus transparent `textarea` overlay in `components/gateway/gateway-form-dialog.tsx`

These implementations diverge in rendering, editing behavior, feature depth, and maintainability. They also make it harder to add validation, save/deploy workflows, and consistent keyboard/search behavior.

## Product Decision

The product decision is to use CodeMirror everywhere in the focused rollout:

- read-only viewing
- editable file surfaces
- config/document text surfaces that already fit this workflow

This is a platform decision, not just a component swap. The system must support:

1. editing documents in the UI
2. saving to an app-managed workspace mirror on disk
3. deploying the saved workspace into a local Claude Code target

## Design System Requirements

The editor must conform to `/home/jmagar/workspace/lab/docs/design-system-contract.md`.

### Core Rule

CodeMirror is the rendering engine. Aurora is the product surface.

The app must not expose raw CodeMirror visual defaults. The editor shell, content chrome, gutters, overlays, popovers, search UI, fold markers, lint panels, autocomplete panels, and focus treatment must all be mapped to Aurora semantic tokens.

### Visual Requirements

- Editor containers use Aurora surface tiers and radius tokens.
- Editor UI chrome uses `Inter`.
- Editable content uses a mono face only where code/document content needs it.
- Colors come from Aurora semantic tokens, not one-off values.
- Focus treatment uses Aurora focus-ring behavior.
- Hover and active states use Aurora interaction tokens.
- Dark mode is the canonical design target; light mode uses the same semantic contract.

### Compliance Areas

The implementation must explicitly style:

- editor background
- line number gutters
- active line
- selection
- cursor
- matching/fold gutters
- search panel
- autocomplete panel
- lint tooltips and markers
- hover tooltips
- inline status chips around save/deploy state

## Rollout Scope

### Included In V1

- marketplace file tree and file viewer/editor panel
- gateway JSON drawer
- TOML-capable config surfaces that already exist in the focused rollout scope
- explicit save into workspace mirror
- explicit local deploy from workspace mirror
- JSON diagnostics/autocomplete
- TOML diagnostics/autocomplete
- frontmatter validation for targeted Claude Code markdown files

### Deferred

- remote targets
- repo ownership and git integration
- generalized migration of all textareas
- deep validation for future `.claude-plugin/marketplace.json`
- deep validation for future `.claude-plugin/plugin.json`

Those deferred manifest targets should still be anticipated in the validator/diagnostics registry.

## Architecture

The implementation should be split into five layers.

### 1. Shared Text Surface

A shared `TextSurface` component wraps CodeMirror and exposes:

- read-only mode
- edit mode
- dirty-state awareness
- language selection
- diagnostics display
- line numbers
- search
- folding
- keyboard shortcut support
- copy support where relevant

This layer owns visual integration with Aurora and shared editor ergonomics. It should not own filesystem, deploy, or marketplace-specific concerns.

### 2. Editor Intelligence Layer

A file-aware registry maps path and language to:

- CodeMirror language support
- schema provider
- validator
- autocomplete provider
- formatting behavior if added

This layer is responsible for:

- JSON parse/schema diagnostics and completions
- TOML parse/shape diagnostics and completions
- Claude frontmatter validation for targeted markdown file classes
- future plugin manifest validation hooks

This registry should be keyed by document path and file type, not by the calling screen.

### 3. Workspace Mirror

The app-managed filesystem location is a workspace mirror.

It must:

- preserve the exact package file tree and relative paths
- store edited file contents as the working copy
- be the destination for explicit `Save`
- remain separate from the local Claude Code target

This layer represents the user’s current working state inside the app, not the deployed runtime state.

### 4. Deploy Adapter

Deploy is a separate layer that syncs from the workspace mirror to the target Claude Code installation.

V1 supports:

- local target only

The abstraction should still separate:

- source workspace mirror
- deployment target resolution
- copy/sync execution
- result reporting

That boundary allows later addition of remote deploy targets without rewriting editor behavior or file storage behavior.

### 5. Feature Integrations

Consumers include:

- marketplace file editor/viewer
- gateway JSON drawer
- any other focused-rollout text/code detail panels

These integrations should consume the shared editor and file APIs rather than reimplementing rendering or document state logic.

## User Experience

### Marketplace Workflow

- The left file tree remains in place.
- The right pane becomes the shared CodeMirror surface.
- Files are editable in this rollout.
- Dirty state appears clearly in the current document header and, where practical, in the file tree.
- `Save` writes the current file into the workspace mirror.
- `Deploy` copies the saved workspace version into the local Claude Code target.

### Gateway Workflow

- The gateway JSON drawer uses the same shared CodeMirror surface.
- The current `pre` plus transparent `textarea` implementation is removed.
- JSON validation and autocomplete are first-class in this surface.

### Mental Model

The user flow is:

1. edit file
2. save working copy
3. deploy working copy

`Save` and `Deploy` are intentionally separate. The system does not conflate draft iteration with live installation state.

## Validation And Diagnostics

### JSON

JSON support should provide:

- parse diagnostics
- schema-aware diagnostics
- autocomplete
- clear invalid/valid state in the surface UI

### TOML

TOML support should provide:

- parse diagnostics
- shape-aware diagnostics where the file target is known
- autocomplete where a structured config target exists

### Claude Frontmatter

For targeted markdown file categories such as agents, skills, and commands, the system should:

- detect the file type from path/context
- parse frontmatter
- validate expected frontmatter structure
- report inline diagnostics without breaking editability

The diagnostics layer should be extensible so future validation for:

- `.claude-plugin/marketplace.json`
- `.claude-plugin/plugin.json`

can be added without redesigning the editor architecture.

## Save Model

`Save` is explicit.

Behavior:

- edits stay in client document state until the user saves
- save writes to the workspace mirror on disk
- save does not deploy
- save should return clear success/error state

Write behavior should be atomic where practical to avoid partial corruption.

## Deploy Model

`Deploy` is explicit and separate from save.

Behavior:

- deploy reads from the workspace mirror
- deploy targets the local Claude Code installation only in this phase
- deploy should refuse to proceed when there are unsaved changes in the package workspace, or require save before continuing
- deploy returns a structured result with changed files, skipped files, and failures

The UI should make the deploy target explicit so users know where files are going.

## API And Server Boundaries

Filesystem and deploy work must go through application server boundaries, not client-side assumptions.

The client editor should call application APIs/server actions for:

- read document
- save document
- enumerate workspace files
- deploy workspace
- inspect deploy target metadata if needed

The browser should never directly own local filesystem semantics.

## Performance And Bundling

CodeMirror language and tooling support should be lazy-loaded by language where practical.

Primary concerns:

- bundle size
- unnecessary editor cost on initial load
- SSR and client-boundary issues in Next.js

The implementation should prefer a stable shared client wrapper that isolates CodeMirror-specific setup from the rest of the app.

## Development Environment Requirement

Local development for `gateway-admin` should run on `0.0.0.0`.

This should be handled as a development/runtime concern in the implementation plan, likely by:

- updating the dev script or standard run command to bind `next dev` on `0.0.0.0`
- keeping `allowedDevOrigins` aligned with that local-network workflow

This requirement is part of the rollout deliverable but remains separate from the editor architecture.

## Risks

- CodeMirror default styling leaking into Aurora surfaces
- feature creep in schema support before the registry abstraction exists
- tying deploy logic to editor widgets
- bundle growth from heavy language/tooling imports
- SSR/client mismatch if editor setup crosses server boundaries
- unclear deploy safety if save state and deploy state are not modeled separately

## Testing Strategy

### Unit Tests

- file path/language registry behavior
- validator routing behavior
- Claude frontmatter validation
- workspace mirror create/read/update behavior
- deploy adapter behavior against a temp local Claude Code target

### Component Tests

- shared `TextSurface` mode switching
- dirty-state rendering
- diagnostics rendering
- save/deploy action state handling at the component boundary

### Integration Tests

- marketplace edit -> save -> deploy
- gateway JSON diagnostics workflow
- TOML diagnostics workflow

One focused browser-level integration flow is worthwhile for the marketplace editing path once the implementation exists.

## Success Criteria

This rollout is successful when:

- a single shared CodeMirror-based component powers both marketplace editing and gateway JSON editing
- marketplace files can be edited and saved into the workspace mirror
- saved marketplace files can be deployed into the local Claude Code target through an explicit deploy action
- JSON and TOML have first-class diagnostics/autocomplete
- targeted Claude frontmatter validation is present
- the editor experience remains aligned with the Labby design system contract
- local development can run on `0.0.0.0`

## Recommended Implementation Direction

Use a shared CodeMirror shell plus marketplace workspace mirror architecture:

- shared `TextSurface`
- diagnostics/schema registry
- workspace mirror abstraction
- local deploy adapter
- feature-specific integrations on top

This provides the right boundary for near-term delivery and keeps the system ready for the later remote-target and repo-backed workflow without forcing those concerns into the first implementation.
