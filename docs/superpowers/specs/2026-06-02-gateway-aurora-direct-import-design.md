# Gateway Aurora Direct Import Design

## Purpose

Lab's Gateway UI currently uses local shadcn/Radix primitives in `apps/gateway-admin/components/ui` plus copied Aurora token helpers in `components/aurora/tokens.ts`. That has been useful, but it lets Lab drift from the canonical Aurora design system. The approved direction is to start using Aurora components directly from `../aurora-design-system`, beginning with the Gateway Core surface.

This spec defines a bounded first migration slice: direct Aurora registry imports for Gateway pages and dialogs, without replacing shared primitives across the whole admin app yet.

## Scope

The first slice covers Gateway Core surfaces:

- Gateway list page and filter rail.
- Gateway table and row action menu.
- Add/Edit Server dialog, including JSON and ENV drawers.
- Gateway detail page controls where they share the same primitive set.
- Gateway-specific auxiliary panels that are part of the normal Gateway workflow, such as tool exposure tables, protected route controls, cleanup/test result panels, and tool search controls.

The first copied Aurora component set is:

- `button`
- `badge`
- `input`
- `label`
- `dialog`
- `tabs`
- `switch`
- `dropdown-menu`
- `table`
- `checkbox`
- `radio-group`
- `field`

Out of scope for the first slice:

- Repo-wide replacement of `apps/gateway-admin/components/ui`.
- Non-Gateway admin pages such as Chat, Marketplace, Nodes, Settings, Setup, and Design System.
- Product workflow redesign of Gateway screens.
- New visual direction beyond adopting canonical Aurora components and tokens.
- Destructive live Gateway operations during verification.

## Source Of Truth

Aurora's source of truth is the sibling checkout:

- Tokens: `../aurora-design-system/registry/aurora/styles/aurora.css`
- UI registry: `../aurora-design-system/registry/aurora/ui/*.tsx`
- Public registry metadata: `../aurora-design-system/public/r/aurora-*.json`

Lab should not hand-port Aurora styles into ad hoc helpers for this slice. The migration should copy the registry component source files into Lab, keep their Aurora semantics recognizable, and update Gateway consumers to import the copied Aurora components directly.

## Install Target

Install the copied Aurora registry files under:

```text
apps/gateway-admin/components/aurora/registry/ui/
```

Install the canonical Aurora stylesheet snapshot under:

```text
apps/gateway-admin/components/aurora/registry/styles/aurora.css
```

The application root should import that stylesheet once from `apps/gateway-admin/app/globals.css` or an equivalent root-only CSS boundary. Lab-only tokens such as live exposure preview colors may remain in `globals.css`, but they must be clearly separated from copied Aurora tokens and named as Lab extensions rather than pretending to be upstream Aurora tokens.

## Import Policy

Gateway files migrated in this slice should import directly from the Aurora registry namespace, for example:

```ts
import { Button } from '@/components/aurora/registry/ui/button'
import { Dialog, DialogContent } from '@/components/aurora/registry/ui/dialog'
```

Do not create a compatibility facade that preserves existing `@/components/ui/*` imports for Gateway. The user selected direct imports. Any component API differences must be handled explicitly in the Gateway files being migrated.

Do not overwrite the existing shared `components/ui` primitives in this first slice. Those primitives are still used by other app surfaces and by components that are not part of Gateway Core.

## API Differences To Handle

Aurora registry components are not drop-in identical to Lab's current primitives. Known differences include:

- Aurora `Button` uses variants such as `aurora`, `neutral`, `rose`, `violet`, `ghost`, `destructive`, and `plain`, and supports `loading`.
- Lab's current `Button` uses variants such as `default`, `outline`, `secondary`, `ghost`, `destructive`, and `link`.
- Aurora `DialogContent` supports `hideClose` and `size`; Lab's current dialog uses `showCloseButton`.
- Aurora components rely on canonical tokens such as `--aurora-overlay`, rose/violet accents, semantic status surface/border/foreground tokens, and Aurora typography variables.
- Some Gateway code currently layers Gateway-specific class helpers on top of local primitives; those helper classes must be reviewed and either retained intentionally or removed when Aurora already provides the behavior.

The implementation should make these differences visible and mechanical. Do not hide them behind a new shared abstraction in this slice.

## Migration Sequence

1. Copy canonical Aurora CSS and the selected UI registry files into the Lab Aurora registry namespace.
2. Add or adjust imports so the copied components resolve within Lab's alias setup.
3. Update Gateway Core files to import directly from the Aurora registry namespace.
4. Remap component variants and props explicitly at each Gateway call site.
5. Remove redundant Gateway-specific styling only when the Aurora component already supplies the same behavior.
6. Keep unchanged product behavior: filtering, searching, JSON/ENV sync, detail navigation, row action menu, and safe tool-search toggle restore behavior must continue to work.
7. Run focused tests and browser verification before expanding the migration.

## Verification

The migration is not complete until all of these pass:

- `just web-build`
- Focused Gateway/admin tests covering at least:
  - `components/gateway/gateway-form-dialog.test.tsx`
  - `components/ui/text-surface.test.tsx`
  - `lib/server/gateway-adapter.test.ts`
  - Any new tests added for Aurora registry import behavior
- `git diff --check`
- The 10-checkpoint Gateway browser audit used in the prior pass:
  - list readiness with real runtime rows
  - search and clear
  - status/source/transport filters
  - density and sort layout stability
  - tool-search toggle with state restoration
  - Add Server dialog and validation
  - JSON drawer character typing without focus loss
  - ENV drawer live sync with JSON/form
  - detail page navigation
  - row action menu without destructive clicks

Desktop and mobile screenshots should be captured for the key states before claiming visual alignment.

## Risks

- A broad primitive replacement would create app-wide churn. This spec intentionally avoids that.
- Direct imports will expose API mismatches in Gateway files. That is expected and should be fixed at call sites.
- Token duplication can continue if Lab keeps redefining copied Aurora variables in `globals.css`. The copied Aurora stylesheet should become the base, with Lab-only extensions separated.
- Some tests may still import `@/components/ui/*` because they test local primitives directly. That is acceptable unless those tests claim to verify Gateway's Aurora usage.

## Success Criteria

The first slice is successful when Gateway Core renders with direct Aurora registry component imports, product behavior is unchanged, and browser evidence shows the Gateway UI remains readable and usable across the audited states.

After this slice succeeds, a follow-up design can decide whether to expand direct Aurora imports to Marketplace, Chat, Setup, Settings, and eventually shared `components/ui` replacement.
