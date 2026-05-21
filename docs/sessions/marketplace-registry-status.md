# Marketplace Registry Status

## Outcome

Marketplace is now the unified surface for registry-backed content in `gateway-admin`.

- MCP Registry is fully integrated into Marketplace, including `sources.list`.
- ACP Registry is fully integrated into Marketplace, including `sources.list`.
- ACP catalog rows now carry Marketplace source identity.
- The admin sidebar no longer shows Registry as a separate destination.
- `/registry` no longer exists as a standalone page; it redirects to `/marketplace`.
- The command palette and design-system command palette data no longer model Registry as a separate destination.

## Notes

- Remaining `/registry` references in `apps/gateway-admin` are internal API/types, registry components, or log fixture data rather than user-facing navigation.
- Standalone registry UI components still exist in the repository, but the retired route no longer renders them.
