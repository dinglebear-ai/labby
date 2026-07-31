# .github/ — CI/CD Workflows

The authoritative CI and release contract is
[`docs/runtime/CICD.md`](../docs/runtime/CICD.md). Keep workflow implementation
details there and keep this file focused on rules for editing `.github/`.

## Fleet invariants

- Fast Linux jobs run on the self-hosted runner farm and include exactly one
  `ci-pool-*` routing label.
- Rust compilation uses `.github/actions/setup-rust-kache`, which connects
  trusted jobs to the shared MinIO cache at `s3.tootie.tv`. Jobs without cache
  credentials fail open to bare Cargo.
- Release builds, container images, Incus images, publishing, signing, and
  attestations run only from `release.published` events on GitHub-hosted
  x86_64 runners.
- Native Windows CI is GitHub-hosted and advisory to the stable `ci-gate`.
- External actions and reusable workflows are pinned to full commit SHAs.
- Fleet contract callers must pass the same exact workflows commit as
  `implementation-ref`.
- Preserve product-specific checks: all-feature Rust tests, feature and
  extracted-crate slices, coverage floors, MCP regressions and conformance,
  Gateway Admin browser tests, Palette checks, npm launcher tests, security
  audits, and Unraid plugin validation.
- `ci-gate` is the stable required aggregate. It accepts required jobs that
  conclude `success` or are intentionally `skipped`.
- Preserve the MSRV command exactly:
  `cargo +1.97.1 check --workspace --all-features --all-targets --locked`.

## Workflow routing

| Surface | Runner |
|---|---|
| Rust compile, test, coverage, security | `[self-hosted, ci-pool-rust]` |
| Node, pnpm, browser, frontend | `[self-hosted, ci-pool-typescript]` |
| policy, labels, drift, metadata, aggregate gates | `[self-hosted, ci-pool-ops]` |
| native Windows advisory checks | `windows-latest` |
| release and publication jobs | pinned GitHub-hosted x86_64 image |

`ci.yml` uses `scripts/ci/changed_paths.py` to route work. Scheduled and manual
runs enable all categories. Pull-request CI validates container and release
source contracts only; it never builds release binaries or container images.

## Release flow

Release Please maintains the version and changelog PR. Publishing the resulting
stable GitHub release triggers the heavy release workflows:

- `release.yml` builds and smokes Linux and Windows archives, builds and scans
  the container, verifies and attaches artifacts, publishes npm and MCP
  Registry metadata, and signs/attests release outputs.
- `build-incus-image.yml` uses the central hosted Incus image workflow and
  publishes checksum-verified image assets plus the rolling Incus alias.

The supported artifacts are Linux x86_64 and Windows x86_64 only. Do not add
other architectures, emulation, cross-platform image matrices, or QEMU setup.

## Editing rules

- Every local job needs a bounded `timeout-minutes`.
- Keep `permissions` least-privileged at workflow and job scope.
- Do not weaken immutable pins, checksum verification, provenance, signing,
  registry visibility checks, or release version lockstep.
- Update `scripts/ci/test_windows_ci_policy.py`,
  `crates/labby/tests/ci_changed_paths.rs`, and `docs/runtime/CICD.md` when a
  workflow contract changes.
- Run Actionlint, focused workflow contract tests, the central fleet policy and
  fleet contract, the forbidden-architecture scan, and `git diff --check`
  before committing.

`AGENTS.md` and `GEMINI.md` in this directory must remain symlinks to this file.
