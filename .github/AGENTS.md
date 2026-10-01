# .github/ — CI/CD Workflows

The authoritative CI and release contract is
[`docs/runtime/CICD.md`](../docs/runtime/CICD.md). Keep workflow implementation
details there and keep this file focused on rules for editing `.github/`.

## Fleet invariants

- All repository-defined CI jobs run on GitHub-hosted runners. Linux jobs use
  the pinned `ubuntu-24.04` image, or `ubuntu-24.04-arm` for native ARM64
  release builds. Windows jobs use `windows-latest`.
- Rust compilation uses `.github/actions/setup-rust-kache` in credentialless
  GitHub-cache mode. Repository-level shared MinIO credentials are forbidden:
  same-repository pull requests can edit workflow content and must never be
  able to name a repository secret that grants shared-cache access.
- Binary/container candidate builds, stateful upgrade qualification, signing,
  and attestations start from immutable stable-version tag pushes on
  GitHub-hosted runners. The GitHub release remains draft until the final
  qualification job promotes it. Incus and registry publishers are callable
  gates that complete before promotion and are covered by aggregate
  reconciliation.
- Native Windows Rust tests and installer contracts are GitHub-hosted and
  manually selected with `workflow_dispatch` and `run_windows=true`. All Windows
  jobs are advisory and excluded from the stable `ci-gate`.
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
| manual advisory native Windows tests, installer, and Palette check | `windows-latest` |
| Linux x86_64 release and publication jobs | pinned GitHub-hosted `ubuntu-24.04` image |
| Linux arm64 release and qualification jobs | native GitHub-hosted `ubuntu-24.04-arm` runner |
| macOS arm64 release and qualification jobs | native GitHub-hosted `macos-15` runner |

`ci.yml` uses `scripts/ci/changed_paths.py` to route work. Scheduled and manual
runs enable all categories. Required CI validates container and release source
contracts. A separate path-filtered workflow builds and smokes the Incus image
when its inputs change.
Fleet policy and repository-contract jobs run on GitHub-hosted runners in
this repository. The contract checks out an immutable central implementation
and invokes the narrow AGENTS-first adapter in `scripts/ci/check_repository_contract.py`.
Preserve its checker digest, complete fleet-driver invocation, Git-index
validation, regression suite, and required aggregate. Do not suppress unrelated
findings or convert a failed contract to success.

## Release flow

Release Please maintains the version and changelog PR, creates the immutable
stable tag, and leaves the GitHub release as a draft. The tag triggers the
heavy candidate workflow:

- `release.yml` builds and smokes Linux and macOS archives, runs N-1 stateful
  upgrade/rollback adapters (including Incus), emits an
  SBOM per subject and a digest manifest, verifies provenance as a consumer,
  publishes npm/GHCR, and only then promotes the draft GitHub release.
- `incus-image.yml` runs only when image inputs change or on manual dispatch.
  Its reusable builder smokes a substrate image without a bundled Labby binary,
  publishes an immutable commit-named release, verifies checksum and provenance,
  and advances the rolling image tag with a rollback receipt.

Releases are created as drafts (`"draft": true` in `release-please-config.json`).
Do not publish one manually: `release.yml` is the sole promotion owner and may
promote only after every declared candidate qualification succeeds.
`release-publish-reminder.yml` surfaces stuck drafts and reconciles published
assets against `release-manifest.json`; it never performs promotion.

ARM64 workflow, installer, and package contracts are explicitly enabled for
Labby through the pinned fleet policy and repository contract. Keep that opt-in
visible when adding ARM64 jobs or artifacts; QEMU and cross-platform emulation
still require a deliberate implementation and verification plan.
The supported release binary artifacts are Linux x86_64, Linux arm64, and macOS arm64.
Windows remains covered by optional manual CI tests but is not a release target.
Keep each release target native to its GitHub-hosted runner; do not add
emulation, cross-platform image matrices, or QEMU setup.

## Editing rules

- Do not override Rust job parallelism with `CARGO_BUILD_JOBS` without an
  explicitly reviewed CI resource-policy change. Preserve the pinned setup
  action and linker/cache contract, and measure the current native workload
  before changing it. Do not justify policy with stale dependency counts or
  old memory estimates: the root manifest now selects the `ring` TLS provider,
  not the previous `aws-lc-rs` default.
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

`CLAUDE.md` and `GEMINI.md` in this directory must remain relative symlinks to `AGENTS.md`.
