# Runtime resource slice

The authenticated `/runtime` page presents a read-only daemon resource sample using
`gateway.host.metrics` with empty params. Rust catalog authority is
`crates/labby-gateway/src/gateway/catalog.rs:1885`: non-destructive and admin-only.
The gateway retains authorization; the UI never treats authentication as admin
permission. A forbidden response clears the console session using the existing
Auth contract.

`crates/labby-gateway/src/gateway/host_metrics.rs` defines the serialized
`HostMetrics` contract. The projection permits only its declared fields, validates
required booleans, bounded text, nonnegative numeric values and nullable counters,
and preserves unsupported values as `Unavailable` instead of zero. CPU is daemon
and observed descendants, disk is the configured data filesystem, and network is
shared namespace traffic. The scope is available in a disclosure. No service
control, remediation, or general host monitoring is implied.

The sampler intentionally waits five seconds. Its shared transport therefore needs
an action-specific eight-second receive deadline, with the ordinary two-second
connect deadline, bounded body, no redirects and no retries retained. Other reads
keep the shorter deadline. A 2.2-second HTTP fixture catches regression to the
ordinary timeout.

The layout follows the mock's runtime/resource grouping: CPU, process memory,
children, data filesystem and namespace traffic. It uses the shared shell,
refresh action and root-owned responsive resource styles. No mock data is embedded
in production modules.

## TDD and verification

User-approved seams: actual HTTP client against isolated loopback fixture and
LiveView visible loading, snapshot, error/recovery and caller-loss behavior.

- First red: `TMPDIR=... mix test test/labby_console/runtime_test.exs`, undefined
  `LabbyConsole.Runtime.metrics/1`; `/tmp/runtime-red.log`.
- LiveView red: `TMPDIR=... mix test test/labby_console_web/runtime_live_test.exs`,
  missing `/runtime` route; `/tmp/runtime-live-red.log`.
- Final focused check:
  `mix test test/labby_console/runtime_test.exs test/labby_console_web/runtime_live_test.exs`
  **5 passed**, seed 171074; `/tmp/runtime-final.log`.
- Coverage includes projection, invalid/oversized/unavailable responses, nullable
  unsupported fields, strangers, forbidden access, slower sampling, stale snapshot
  removal, recovery, disabled loading refresh and expiry during pending sampling.
- `e2e/runtime.spec.mjs` authors browser sign-in, isolated caller, refresh failure,
  unsupported values, recovery, expired access and 320px overflow checks.
  The coordinator owns the serial browser run; this agent did not run Playwright.

These checks use local fixtures. No live gateway request, production credential,
configuration mutation, deployment or Git operation was performed.
