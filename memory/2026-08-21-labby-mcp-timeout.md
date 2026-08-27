# Labby MCP timeout and macOS test recovery

Date: 2026-08-21

## Symptom

Provider-managed Core installs downloaded and unpacked successfully far enough
to exceed Labby's 30-second outer HTTP request deadline. The provider relay
budget was already five minutes, but the Labby bridge returned HTTP 504 first.
The failing request began at 23:05:20 UTC and the bridge timed out at about
23:05:50 UTC.

## Root cause

The outer Axum router applied one 30-second `TimeoutLayer` to every route,
including `/mcp`. That budget was shorter than the existing provider relay
budget used by long-running VM installs.

## Fix

`/mcp` and `/mcp/*` now use a five-minute request budget. Other HTTP routes
retain the 30-second timeout. A router regression test covers both MCP path
forms and the ordinary API default.

Commits:

- `dd140e9d fix(api): extend MCP request timeout for long installs`
- `bd77367b fix(mac): stabilize platform-sensitive tests`

The macOS fixes cover relative system symlink validation, short Unix-socket
test paths, macOS directory-open semantics, the correct HTTP environment
variable names, `/private/var` path canonicalization, isolated OAuth tracing
tests, deterministic Code Mode test state, and stale palette-cache ownership.

## Verification

`cargo test -p labby --all-features` passed:

- 1,284 library tests
- all package integration suites, including the 10 Unix-listener tests and
  14 upstream OAuth tests
- exit status 0

`cargo fmt --all -- --check` and `git diff --check` also passed.

## Local deployment

The release-fast binary was installed for the local macOS LaunchAgent. The
service is running on its configured local port, `/health` returned
`status: ok`, and a fresh gateway refresh reports the QA provider connected
with 41 tools and one skill resource.

## VM validation

The provider recovery list became empty and the previously affected capacity
was released. A fresh licensed reservation was acquired, booted, and reached
`artifactInstall: ready`. The install request reached the provider through the
rebuilt gateway without a 30-second bridge timeout, but the provider rejected
`core-pr-809` immediately because that image is not configured. No install
operation was created. The reservation was then released through the supported
cleanup path with a cold rollback and no recovery work.

The remaining prerequisite for a successful VM install is an approved,
provider-configured `core-pr-809` image or a complete caller-pinned Core image
descriptor. No unrelated Core image was substituted.
