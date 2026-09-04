# .github/ — CI/CD Workflows

The authoritative CI and release contract is
[`docs/runtime/CICD.md`](../docs/runtime/CICD.md). Keep workflow implementation
details there and keep this file focused on rules for editing `.github/`.

## Fleet invariants

- All repository-defined CI jobs run on GitHub-hosted runners. Linux jobs use
  the pinned `ubuntu-24.04` image, and Windows jobs use `windows-latest`.
- Rust compilation uses `.github/actions/setup-rust-kache`, which connects
  trusted jobs to the shared MinIO cache identified by the org variable
  `KACHE_S3_ENDPOINT`. Jobs without cache
  credentials fail open to bare Cargo.
- Release builds, container images, Incus images, publishing, signing, and
  attestations run only from `release.published` events on GitHub-hosted
  runners.
- Native Windows CI is GitHub-hosted and advisory to the stable `ci-gate`.
- External actions and reusable workflows are pinned to full commit SHAs.
- Fleet contract callers must pass the same exact workflows commit as
  `implementation-ref`.
- Preserve product-specific checks: all-feature Rust tests, feature and
  extracted-crate slices, coverage floors, MCP regressions and conformance,
  Gateway Admin browser tests, Palette checks, npm launcher tests, security
  audits, and Unraid plugin validation.
- `ci-gate` is the stable required aggregate. It accepts required jobs that
  conclude `success` or are intentionally `skipped`. `changes` and
  `fleet-policy` are the exceptions: neither has an `if:`, so both must
  conclude `success`. A skipped `changes` also empties every gate expression,
  which would skip every gated job and leave the run vacuously green.
- Changed-path routing fails open. On pull requests the trusted classifier
  comes from the base commit while `ci.yml` comes from the merge ref, so a
  gated key the classifier does not emit is forced to `true` and reported
  through the `gate_key_drift` output — never left to skip silently. The
  branch's own classifier is unioned in over the trusted changed-file list so
  new path mappings route correctly, in the broadening direction only.
- Pinning the classifier to the base commit is an accident guard, not a
  security boundary: on a same-repo pull request the gate expressions and the
  `changes` job's `outputs:` block come from the merge ref and are
  branch-controlled. Do not describe it as preventing a branch from rerouting
  its own CI.
- `.github/workflows/protected-docs.yml` is intentionally different: it uses
  `pull_request_target`, checks out only `github.event.pull_request.base.sha`,
  never executes pull-request code, and receives only read permissions. Keep
  `Protected docs guard` as a separate required branch-protection context.
  Changes below `docs/sessions/` or `docs/superpowers/` require the
  maintainer-applied `protected-docs-approved` label.
- Preserve the MSRV command exactly:
  `cargo +1.97.1 check --workspace --all-features --all-targets --locked`.

## Workflow routing

| Surface | Runner |
|---|---|
| Rust compile, test, coverage, security | `ubuntu-24.04` |
| Node, pnpm, browser, frontend | `ubuntu-24.04` |
| policy, labels, drift, metadata, aggregate gates | `ubuntu-24.04` |
| native Windows advisory checks | `windows-latest` |
| release and publication jobs | pinned GitHub-hosted x86_64 image |

`ci.yml` uses `scripts/ci/changed_paths.py` to route work. Scheduled and manual
runs enable all categories. Pull-request CI validates container and release
source contracts only; it never builds release binaries or container images.
The reusable fleet policy and repository contract remain organization-managed
workflow calls. Their execution environment is owned by the central workflows
repository.

## Release flow

Release Please maintains the version and changelog PR. Publishing the resulting
stable GitHub release triggers the heavy release workflows:

- `release.yml` builds and smokes Linux, macOS, and Windows archives, builds and
  scans the container, verifies and attaches artifacts, publishes npm and MCP
  Registry metadata, and signs/attests release outputs.
- `build-incus-image.yml` uses the central hosted Incus image workflow and
  publishes checksum-verified image assets plus the rolling Incus alias.

Releases are created as drafts (`"draft": true` in `release-please-config.json`)
so publication stays a human decision. Do not make any workflow publish a draft
release; `release.yml` asserts it was invoked from an already-published release.
`release-publish-reminder.yml` only surfaces pending drafts as an issue.

ARM64 workflow, installer, and package contracts are explicitly enabled for
Labby through the pinned fleet policy and repository contract. Keep that opt-in
visible when adding ARM64 jobs or artifacts; QEMU and cross-platform emulation
still require a deliberate implementation and verification plan.
The supported binary artifacts are Linux x86_64, macOS arm64, and Windows
x86_64. Keep each target native to its GitHub-hosted runner; do not add
emulation, cross-platform image matrices, or QEMU setup.

## Editing rules

- Never set `CARGO_BUILD_JOBS` on a Rust job. Cargo forwards it to every build
  script as `NUM_JOBS`, and `aws-lc-sys` compiles 414 C and 902 assembly
  sources through the `cc` crate. Kache cannot cache those sources because it
  wraps `rustc`, not `cc`. Hosted runner measurements show that a full
  workspace build linking all 15 test harnesses peaks at 5.03 GiB, while
  `nextest run --workspace --all-features` peaks at 2.44 GiB. Use lld to hold
  link memory down.
- Every local job needs a bounded `timeout-minutes`.
- Keep `permissions` least-privileged at workflow and job scope.
- Do not weaken immutable pins, checksum verification, provenance, signing,
  registry visibility checks, or release version lockstep.
- A new routing key must be added to `OUTPUT_KEYS` in
  `scripts/ci/changed_paths.py` **and** to the `changes` job's `outputs:` block,
  forwarding the identically-named classify output, before anything gates on
  it. A gate on an undeclared or misspelled key reads as the empty string and
  skips the job; the classify step and
  `crates/labby/tests/ci_changed_paths.rs` both fail the build on that.
- Gates must use `needs.changes.outputs.<key>`. The bracket form is invisible
  to the classify step's reconciler.
- `ci-gate` must aggregate every non-advisory job in both its `needs:` list and
  its `require_*` assertions; a job in one but not the other cannot fail the
  build.
- Update the focused CI policy tests under `scripts/ci/`,
  `crates/labby/tests/ci_changed_paths.rs`, and `docs/runtime/CICD.md` when a
  workflow contract changes.
- Run Actionlint, focused workflow contract tests, the central fleet policy and
  fleet contract, the architecture-policy scan, and `git diff --check`
  before committing.

`AGENTS.md` and `GEMINI.md` in this directory must remain symlinks to this file.
