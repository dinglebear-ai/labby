# `components/ui/` — shadcn primitive customization rule

These are Labby's pre-registry customized shadcn primitives. Keep them shallow while they remain in use. Shared Aurora primitives are canonical in the standalone `dinglebear-ai/aurora` registry and install under `components/ui/aurora/*`; new reusable Aurora behavior belongs upstream there, not in a second Labby-only fork. Existing `components/ui/*` primitives may migrate incrementally after their public API and call sites are reconciled.

## What may be baked into a primitive

- **Brand identity tokens**: font family, weight, tracking, color tokens (`font-display`, `text-aurora-*`, `bg-aurora-*`).
- **Default variant selection**: e.g. `cardVariants` mapping Aurora tier names.

Example: `CardTitle` defaults to `font-display` because every Aurora card header should use Manrope. Forgetting it at the call site is a contract violation; baking it in is safe-by-default.

## What must NOT be baked in

- **Layout / spacing**: `gap-`, `p-`, `m-`, `grid-cols-*`. These are call-site decisions.
- **Sizing / type ramp**: `text-xl`, `text-3xl`, `h-`, `w-`. Use `AURORA_DISPLAY_*` token strings at the call site instead.
- **Behavioral coupling**: callbacks, refs, anything that changes the primitive's contract.

Rule of thumb: if the change describes *what the brand looks like*, bake it in. If it describes *where this instance sits in this layout*, leave it for the caller.

## Local Checks

Run from `apps/gateway-admin/`:

- `pnpm build`
- `pnpm test`
- `pnpm lint`
