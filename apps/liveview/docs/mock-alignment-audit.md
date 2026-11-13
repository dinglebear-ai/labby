# Implemented screen alignment audit

Scope: the real LiveView console's existing Gateway, server details/catalog/runtime,
host Runtime, Clients, Diagnostics, Usage, Activity and Logs screens. This is not
an assertion that the entire mock's workspace functionality is implemented.

## References checked

- Live mock `https://phx-book.dinglebear.ai/labby/gateway`, including Fetch Overview,
  inspected with a separate named browser session on 2026-10-27.
- Mock source `lib/phx_book_web/components/labby_gateway_workspace.ex` and
  `priv/static/assets/css/labby-gateway-workspace.css` in the separate phx-book checkout.
- Reference screenshots retained locally at `/tmp/labby-mock-gateway-audit.png` and
  `/tmp/labby-mock-server-audit.png`. Mock checkout was not modified.

## Applied alignment

- Keep the mock's 208px navy navigation shell, fold brand, 24px content gutters,
  compact type, cyan navigation icons and restrained semantic status colors.
- Current navigation uses a straight accent, including the mobile layout. Focus
  accents are straight inset lines, with no curved halo.
- Inventory now has the mock's usable search affordance, filtered/total count and
  no-match state. Search only filters the caller's loaded snapshot, preserves the
  query on refresh, and does not add API traffic or imply global discovery.
- Server details link directly to the implemented Overview, Catalog and Runtime
  screens. Configuration IDs and upstream names retain their distinct routing.
- Unimplemented mock controls (add/start/stop/re-auth, bulk selection and workspace
  pages) are not displayed as dead buttons.

## Verification

TDD: server section navigation failed before implementation; inventory search
failed before implementation. Focused seven LiveView regressions then passed
(seed 977223). `e2e-real/visual-alignment.spec.mjs` checks ten implemented screens
against the real backend at desktop and 320px: shared current navigation,
full-width panel alignment, no page overflow, functional inventory search, and
20 screenshot attachments. Coordinator owns the full serial real-browser run;
adding the test does not itself establish browser success.

## Remaining differences

The mock has broader content and a different information architecture: Overview
combines several real read models, Observe combines activity/logs/traces, and the
mock server view has more tabs and mutation controls. The current real console
keeps its implemented read-only slices independently navigable. API availability,
caller scope, empty results and actual diagnostics remain authoritative; mock
sample counts, warning banners and fake clients are never copied. Pixel parity
is intentionally not asserted between different runtime content. New workspace
pages and missing actions remain future product work, not completed alignment.

Coordinator render review reproduced and corrected two cascade regressions:
row-level muted text overrode connected/disconnected semantic colors, and the
legacy Refresh ID rule shrank its icon to 10px. Real visual assertions now require
a readable icon (at least 16px) and distinct connected/disabled colors. These checks
failed against the previous rendered styles before the corrective CSS was added.

## Next slice: Overview

`/overview` now combines the existing caller-scoped inventory and last-24-hour
usage projections. It reuses the shell, semantic tokens and refresh action, with
stacked cards at mobile widths and links into implemented details. Independent
async reads keep a failed usage panel from hiding successful inventory. Refresh
clears old summaries; session expiry removes data and returns to sign-in.
The page does not infer readiness from connections or add mock sample values.

The two new LiveView regressions failed before the route existed, then passed.
A real-backend journey covers anonymous access, navigation, loaded summaries,
refresh, desktop/320px overflow and screenshots. This is the first Overview slice;
charts, leaders and the full mock dashboard composition remain future work.
