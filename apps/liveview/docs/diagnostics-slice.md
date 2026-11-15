# Authenticated diagnostics slice

`/diagnostics` reads `POST /v1/doctor` with `{"action":"system.checks","params":{}}` through the bounded, caller-scoped `Auth.read_action/4` transport. It never invokes repair, audit streaming, credential changes, or gateway mutations.

Contract authority: `crates/labby/src/dispatch/doctor/catalog.rs` declares `system.checks` read-only; `dispatch.rs` returns `Report`; `types.rs` defines `findings` with `service`, `check`, `severity` (`ok`, `warn`, `fail`), and `message`. `system.rs::run_system_checks` performs local presence, filesystem and toolchain probes; it does not repair configuration. These checks describe the configured gateway host, not the console host.

The console bounds reports to 500 findings, validates field types/severity/length, projects only contract fields, and HTML-escapes displayed values. Loading, empty, unavailable and explicit refresh are visible. Failed refreshes clear prior findings. Session expiry or revoked access redirects to sign-in. Findings can contain host paths and environment variable names and are restricted to the authenticated caller.

Regression seams: local authenticated HTTP fixture plus visible LiveView behavior. `mix test test/labby_console/diagnostics_test.exs test/labby_console_web/diagnostics_live_test.exs`. Browser journey: `npx playwright test e2e/diagnostics.spec.mjs` using the repository's isolated fixture launcher. Tests do not establish live service or hosted CI acceptance.
