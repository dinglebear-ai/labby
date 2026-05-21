# Mobile UI Fixes - 2026-05-07

## Summary

Addressed the mobile UI issues found during the `agent-browser` audit across the marketplace and the rest of the gateway-admin pages.

## Issues Addressed

- `/dev` and `/dev/marketplace` were intercepted by Axum mockup routes instead of serving the Next pages. The mockup routes now live under `/dev/mockup`.
- Marketplace mobile cards now wrap long package names and metadata safely at 320px.
- Marketplace no longer renders the full 8k+ catalog into the DOM at once; the visible list is capped to the first 200 filtered results with a narrowing hint.
- Marketplace mobile filters now open in a bottom sheet instead of pushing the result list down inline.
- Marketplace filter/status pills were adjusted for stronger mobile contrast.
- Marketplace summary chips now reflect active search and filters while preserving the selected lens behavior.
- `/nodes` header actions now fit at 320px by switching nonessential labels to icon-only controls on mobile.
- `/settings/*` now uses a mobile section select instead of a cramped horizontal rail.
- Design system table sample no longer forces a mobile min-width overflow.
- Floating chat action is icon-only on mobile and page content gets bottom padding so it does not cover controls.
- Added semantic `h1` coverage for `/gateways`, `/logs`, `/settings/*`, `/chat`, and marketplace plugin detail.

## Key Files Changed

- `apps/gateway-admin/components/marketplace/marketplace-list-content.tsx`
- `apps/gateway-admin/components/marketplace/plugin-detail-content.tsx`
- `apps/gateway-admin/components/nodes/nodes-page.tsx`
- `apps/gateway-admin/components/settings/SettingsRail.tsx`
- `apps/gateway-admin/components/admin-layout-client.tsx`
- `apps/gateway-admin/components/floating-chat-fab.tsx`
- `apps/gateway-admin/components/design-system/data-display-section.tsx`
- `apps/gateway-admin/components/gateway/gateway-list-content.tsx`
- `apps/gateway-admin/components/logs/log-console.tsx`
- `apps/gateway-admin/components/chat/chat-shell.tsx`
- `apps/gateway-admin/app/(admin)/settings/*`
- `crates/lab/src/api/router.rs`

## Verification

Passed:

```bash
pnpm test -- --runInBand
pnpm build
cargo fmt --check
cargo check --workspace --all-features
cargo build -p labby --all-features
```

Evidence:

- `pnpm test -- --runInBand`: 283 tests passed.
- `pnpm build`: Next static build completed successfully.
- `cargo check --workspace --all-features`: completed successfully.
- `cargo build -p labby --all-features`: completed successfully.
- Live service was rebuilt and restarted through Docker Compose.
- `agent-browser` mobile viewport `320x844` was used to verify:
  - `/marketplace`
  - `/dev/marketplace`
  - `/nodes`
  - `/settings/core`

Screenshots saved during verification:

- `/tmp/lab-marketplace-mobile-after.png`
- `/tmp/lab-marketplace-filters-mobile.png`
- `/tmp/lab-nodes-mobile-after-2.png`
- `/tmp/lab-settings-core-mobile-after.png`
- `/tmp/lab-dev-marketplace-mobile-after.png`

## Notes

The working tree already contained unrelated marketplace/API/runtime edits before this pass. Those were preserved and not reverted.
