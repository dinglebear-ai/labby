# Aurora component contract

Shared Aurora design-system constants. Source of truth for token naming is `app/globals.css` (CSS vars) and `docs/design/design-system-contract.md` (semantics).

**One-way dependency rule:** `components/aurora/**` must NOT import from `components/ui/**`. Token module depends on nothing in the primitive tree. Primitives may depend on this module.
