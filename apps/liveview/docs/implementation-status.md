# LiveView implementation status

Source review: 2026-11-10 at `5cb9dd2f7`. This inventory describes implemented
code. Test results belong to the candidate-specific PR verification receipt.

Supersedes the 2026-11-08 review at `583d385f2`, which predated the caller-bound
action transport, the Projects lifecycle and the Access workspace.

## Implemented boundary

The console is a separate Phoenix application beside the Rust gateway. It has
sixteen authenticated screens, public readiness, and bearer-session
sign-in/sign-out including provider start/poll/cancel. Rust remains authoritative
for identity, scope, validation, configuration, domain operations and
persistence. The embedded Next.js UI and desktop shell remain in place. There is
no deployment or routing cutover here.

The protected transport forwards each caller's gateway cookie and inspected CSRF
token. It bounds requests/responses, disables retries and redirects, and projects
responses for display. The in-memory session vault is for a single console
instance; a restart requires callers to sign in again.

Reads and mutations are separately allowlisted in `LabbyConsole.GatewayAction`.
A disallowed action is never sent. Mutations run as supervised, caller-owned
operations with bounded admission, cancellation and restart recovery.

## Wired contract surface

| | Count |
| --- | --- |
| Allowlisted reads | 21 |
| Allowlisted mutations | 24 |
| Total actions wired | 45 of 320 in the contract catalog (14%) |
| Services touched | 5 of 20 (`gateway`, `projects`, `access`, `doctor`, `server_logs`) |

Mutations by service: `gateway` 4 (test/add/update/remove), `projects` 4
(create/update/archive/activate), `access` 16 (team, membership, role,
invitation, project assignment, platform admin, gateway credential).

Services with no console consumer: `artifacts` (43), `setup` (29),
`dev_containers` (17), `snippets` (16), `tasks` (16), `stash` (15), `agents` (12),
`browser` (11), `bundles` (10), `jobs` (7), `sources` (6), `uploads` (5),
`fs` (4), `artifact_publish` (3).

## Routes

`/` integration, `/connection`, `/overview`, `/observe`, `/gateway`,
`/gateway/servers/:id`, `/gateway/servers/:id/catalog`,
`/gateway/upstreams/:name/runtime`, `/gateway/upstream-runtime`, `/runtime`,
`/clients`, `/usage`, `/activity`, `/logs`, `/diagnostics`, `/projects`,
`/access`, plus the sign-in/sign-out controller actions.

## Comparison with the mock

The reference is the separate `phx-book` prototype. Its Labby routes start at
`lib/phx_book_web/router.ex`; native pages use `PhxBookWeb.Labby*Live` and shared
`labby_*` components/styles. Direct `/labby/overview` and `/labby/gateway` routes
show the mock without Storybook chrome. The prototype uses session-local sample
data and simulated domain actions, not the real gateway.

The mock has eighteen named Labby areas and 98 reference workflows. The real
console now covers parts of six areas. This is a scope count, not a completion
percentage: the areas have different sizes and navigation structures.

| Mock area | State | Real console | Remaining scope |
| --- | --- | --- | --- |
| Overview | partial | `/overview`: inventory, last-24-hour usage, drilldowns, independent loading/error states, refresh | Charts, leaders and full dashboard composition |
| Gateway | partial | `/gateway` inventory/search; server details, retained tool/resource/prompt catalog, passive server runtime; `/runtime`, `/clients`; add/update/remove/test with normalized-save reconciliation; OAuth handoff | Start/stop/reload, configuration editor/import, bulk actions, remaining re-auth journeys |
| Observe | partial | `/observe` workspace plus `/usage`, `/activity`, `/logs`: usage windows, opaque call pagination, bounded log polling, positive log filters | Traces and richer inspectors |
| Projects | partial | `/projects`: list/get/create/update/archive/activate, Team membership reads, caller-isolated filter/layout preferences, uncertain-state retention until reads reconcile | Remaining mock dialogs and workspace composition |
| Access | partial | `/access`: team, membership, role, suspend/activate, invitations, project assignment, platform admin, gateway credential bind/revoke; secret redaction | Remaining administration journeys and role matrix coverage |
| Diagnostics | partial | `/diagnostics`: gateway system checks | Repairs and setup/audit operations |
| Discover, Create, Library | open | No workspace implementation | Discovery/install, resource wizard, source editor, builders, artifact management and publish flows |
| Settings, Stash, Tasks, Agents | open | No workspace implementation | Their respective management workflows |
| Browser, Setup, Depot, Tools, Environments | open | No workspace implementation | Browser pairing, onboarding, depot, workspace tools and environment operations |

Public readiness does not implement Setup. Direct gateway mutations performed by
test setup are not console UI features.

## Verification coverage and limits

- 397 ExUnit cases across 62 files cover public transport, authentication, HTTP
  contracts, LiveView state, caller isolation, invalid responses, bounded reads,
  refresh, expiry, action admission, payload bounds, deadlines, cancellation and
  guardian-restart recovery.
- 110 Playwright cases across 26 fixture specs and 63 real-backend files.
- Fixture browser journeys cover controlled failures, revocation and recovery.
- The separate real-backend suite builds the selected source and exercises actual
  Rust Labby plus a local MCP protocol peer: discovery/execution, populated call
  history/cursors, daemon restart, scoped credentials, CSRF and revocation.
- Browser runs use one worker, no retries, a three-failure stop budget and a
  ten-minute global timeout. Visual checks cover desktop and narrow mobile.
- Local protocol-peer coverage does not qualify external OAuth, third-party
  providers, all member roles, clustered sessions or a deployed environment.

Packaging remains `blocked_on_ui_parity`: no native OTP release, embedded
manifest, extraction, supervision or routing work has started.

The companion CI workflow is advisory and separate from required `ci-gate`.
Historical test receipts in this directory apply only to their recorded source;
they do not prove a later candidate passed.
