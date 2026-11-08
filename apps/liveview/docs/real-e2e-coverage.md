# Real-backend coverage and mock alignment

The default `just liveview-e2e` builds the current Rust binary and LiveView assets,
then starts owned loopback services: supervisor control (4297), LiveView (4298),
Labby (4299), an actual MCP protocol peer (4300), and a separate Labby instance
with a real restricted product credential (4301). No Labby API responses or usage
records are mocked or inserted into a database by the browser suite.

## Feature boundaries

- Individual read-model journeys cover inventory, details, server runtime,
  client observations, diagnostics, host metrics, usage aggregates, retained calls,
  and server logs. They each authenticate independently.
- The local MCP peer advertises two tools, a resource and a prompt. Tests discover
  it through Labby, execute both successful and error results, populate actual
  retained usage, filter calls, and exercise the opaque next-page cursor.
- Offline/recovery checks disable only the owned protocol peer. Catalog retention
  is kept distinct from transport liveness. Active `gateway.status` refresh is
  explicit; `gateway.reload` alone preserves an unchanged lazy pool.
- A restart journey updates a disabled server through `gateway.update`, restarts
  the actual owned daemon, verifies the saved label, and verifies the old browser
  authority must authenticate again. The label is then restored for other tests.
- Real anonymous, invalid-credential, administrator, CSRF, cookie-replay and
  credential-revocation paths are covered. The read-only product credential is
  bootstrapped via public CLI/proof APIs and denied administrator log access.
  This proves a real scope ceiling, not every organization/project member role.
  The current console accepts static setup credentials; project-credential login
  remains unsupported and is tested to fail closed.
- Visual checks traverse ten implemented screens at desktop and 320px, verify
  panel alignment/navigation/overflow, exercise inventory search, and attach
  twenty screenshots. Mock source and actual mock render review are recorded in
  `mock-alignment-audit.md`. Different real/sample contents are not pixel goldens.

## Bounds and diagnostics

One browser worker, no retries, a ten-minute global deadline, and a three-failure
stop budget. MCP pagination seeding is capped at thirty calls; discovery retries
are bounded. Startup readiness and owned child termination have deadlines. The
restart control is loopback-only, protected by a per-run key, and limited to three
restarts. It is never included in the production LiveView application.

Each launcher process retains at most 256 KiB of redacted console diagnostics.
A private `diagnostics.json` records binary SHA-256, stages, owned PIDs/ports,
startup failure identifiers, and cleanup receipts. Missing binary and readiness
failures have deterministic regression tests. No production token is inherited.
The private state roots still contain synthetic credentials/access databases and
must not be uploaded as report artifacts. The restricted proof/credential files
are removed only after supported cleanup confirms revocation; failed cleanup is
reported and retained. Shared processes, state, caches and services are untouched.

## Remaining product scope

The mock includes Create, Discover, Library, Depot, Projects, Tasks, Agents, Stash,
access administration, environments and other workspace experiences which are not
implemented in this console. Missing server mutation controls, traces, editor and
builder flows are not implied by passing read-model tests. External OAuth and real
third-party upstream providers remain outside this local integration suite.

## Integration findings corrected

The first real populated-catalog run showed that unchanged `gateway.reload` is not
an active discovery operation. The peer setup now uses `gateway.status` before a
bounded retained read. A populated activity run then reproduced a UI refresh bug:
its time-window endpoint stayed at page-open time and excluded newly executed
calls. Refresh now advances the endpoint and starts a fresh page; next-page reads
keep their stable cursor window. Both real-browser and visible LiveView regressions
cover the behavior. Native activity selectors also have explicit accessible names.
Fixture and real Playwright output directories are separate so one run does not
remove the other's captures.

## Local acceptance, 2026-10-27

Verified in the working tree based on `fcf397825f5a387582e0422473a615e93ba4deb2`
(the console work remains uncommitted):

- `just liveview-test`: 73 regressions passed.
- Launcher/MCP Node regressions: 4 passed before the real suite.
- `just liveview-e2e`: 20 real-backend Playwright journeys passed, including the
  visual assertions and 24 desktop/mobile screenshots.
- `just liveview-browser-fixtures`: 13 controlled fixture browser checks passed.

Assets were built by the default real-E2E pretest hook. Both owned Labby daemons
were stopped and restricted bootstrap cleanup was confirmed. Hosted CI, production
services, external identity providers, and the full member-role matrix were not
qualified by this local run.
