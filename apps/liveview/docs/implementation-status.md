# LiveView implementation status

Source review: 2026-11-08. This inventory describes implemented code. Test results
belong to the candidate-specific PR verification receipt.

## Implemented boundary

The console is a separate Phoenix application beside the Rust gateway. It has
eleven authenticated read-model screens, public readiness, and bearer-session
sign-in/sign-out. Rust remains authoritative for identity, scope, validation,
configuration, domain operations and persistence. The embedded Next.js UI and
desktop shell remain in place. There is no deployment or routing cutover here.

The protected transport forwards each caller's gateway cookie and inspected CSRF
token. It allows only named reads, bounds requests/responses, disables retries
and redirects, and projects responses for display. The in-memory session vault
is for a single console instance; a restart requires callers to sign in again.

## Comparison with the mock

The reference is the separate `phx-book` prototype. Its Labby routes start at
`lib/phx_book_web/router.ex`; native pages use `PhxBookWeb.Labby*Live` and shared
`labby_*` components/styles. Direct `/labby/overview` and `/labby/gateway` routes
show the mock without Storybook chrome. The prototype uses session-local sample
data and simulated domain actions, not the real gateway.

The mock has eighteen named Labby areas. The real console covers parts of four
areas, spread across eleven read-model screens. This is a scope count, not a
completion percentage: the areas have different sizes and navigation structures.

| Mock area | Real console | Remaining scope |
| --- | --- | --- |
| Overview | `/overview`: inventory and last-24-hour usage, independent loading/error states, refresh and detail links | Charts, leaders and full dashboard composition |
| Gateway | `/gateway`: inventory/search; server details, retained tool/resource/prompt catalog, passive server runtime; `/runtime` and `/clients` | Add/edit/delete, start/stop/reload, OAuth/re-auth, configuration editor/import and bulk actions |
| Observe | `/usage`, `/activity`, `/logs`: usage windows, opaque call pagination and bounded log polling | Unified Observe workspace, traces and richer inspectors |
| Diagnostics | `/diagnostics`: gateway system checks | Repairs and setup/audit operations |
| Discover, Create, Library | No workspace implementation | Discovery/install, resource wizard, source editor, builders, artifact management and publish flows |
| Settings, Stash, Tasks, Projects, Agents | No workspace implementation | Their respective management workflows |
| Access, Browser, Setup, Depot, Tools, Environments | No workspace implementation | Administration, browser pairing, onboarding, depot, workspace tools and environment operations |

Authentication is implemented, but it does not implement the mock's Access
administration workspace. Public readiness does not implement Setup. Direct
gateway mutations performed by test setup are not console UI features.

## Verification coverage and limits

- ExUnit covers public transport, authentication, HTTP contracts, LiveView state,
  caller isolation, invalid responses, bounded reads, refresh and expiry.
- Fixture browser journeys cover controlled failures, revocation and recovery.
- The separate real-backend suite builds the selected source and exercises actual
  Rust Labby plus a local MCP protocol peer: discovery/execution, populated call
  history/cursors, daemon restart, scoped credentials, CSRF and revocation.
- Browser runs use one worker, no retries, a three-failure stop budget and a
  ten-minute global timeout. Original ten-screen visual checks and the separate
  Overview journey cover desktop and narrow mobile layouts.
- Local protocol-peer coverage does not qualify external OAuth, third-party
  providers, all member roles, clustered sessions or a deployed environment.

The companion CI workflow is advisory and separate from required `ci-gate`.
Historical test receipts in this directory apply only to their recorded source;
they do not prove a later candidate passed.
