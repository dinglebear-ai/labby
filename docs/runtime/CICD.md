---
title: "CI/CD"
created: "2026-07-30"
updated: "2026-09-03"
---

# CI/CD

Last updated: 2026-09-03

This document is the authoritative contract for CI, release, and artifact delivery in Labby. All pipeline implementations must conform to this spec.

## CI Path Routing

`ci.yml` starts with a `changes` job that runs `scripts/ci/changed_paths.py`.
That classifier maps the changed file list into stable routing categories:
`all`, `docs`, `docs_check`, `workflow`, `rust_compile`, `rust_test`, `web`,
`palette`, `npm`, `docker`, `security`, `release`, and `unraid`. Scheduled and
manual runs enable every category so periodic/manual validation stays broad.

On pull requests the `changes` job runs the classifier from the pull request's
**base commit** rather than the branch's own copy. Be precise about what that
buys. GitHub runs the workflow file itself from the merge ref, so on a
same-repo pull request the gate expressions, this job's `outputs:` block, and
the tests that police them are all branch-controlled. Pinning the classifier
stops a branch from *accidentally* rerouting its own CI while editing path
rules; it is not a control against a branch that sets out to. The controls for
that are branch protection and review on `.github/**` and `scripts/ci/**`.

The workflow-policy and repository-contract checks run locally on
GitHub-hosted runners. The repository contract still checks out its immutable
implementation from the pinned workflows revision, but the validation job
itself does not depend on the self-hosted fleet. Keeping these checks in the
caller makes the runner boundary visible and testable in this repository.
The caller checkout lives in `caller/`, not `target/` or `vendor/`: the pinned
checker excludes Cargo manifests beneath those build/dependency directory names.

That window: `ci.yml` always comes from the merge ref, so a pull request that
adds a routing key gates on a key the trusted classifier cannot emit, and every
already-open pull request against an older base hits it too.

That window must fail **open**. An unknown output evaluates to the empty
string, and `'' == 'true'` is false — so without reconciliation the gated job
skips, and `ci-gate` accepts the skip as intentional. The `classify` step
therefore reconciles what the classifier emitted against two key sets it reads
back out of `ci.yml`:

- `needs.changes.outputs.<key>` — what other jobs gate on.
- `steps.classify.outputs.<key>` — what the `changes` job forwards as a job
  output.

A gated key the trusted classifier did not emit — or emitted with a value other
than `true`/`false`, which fails `== 'true'` just as an absent key does — is
forced to `true`, so the job runs. Those keys are annotated as warnings and
exported as the `gate_key_drift` output, which `ci-gate` reports. Drift clears
on its own once the base branch carries the new key.

Everything else in that step fails **closed**, because it cannot be repaired by
running more jobs:

- A gate with no matching `steps.classify.outputs.<key>` forward reads as the
  empty string no matter what the classifier emits. That is an authoring bug,
  so the step fails the build and names the key.
- If key enumeration itself fails — `ci.yml` unreadable, moved, or no longer
  matching the expression form — the step fails rather than quietly
  reconciling nothing and reporting an all-clear.
- `ci-gate` turns drift into an **error** on non-`pull_request` events, where
  the classifier comes from the same commit as `ci.yml`: there, drift means the
  two genuinely disagree.

Three rules keep this contract honest:

- Never gate a job on a key that is not declared in the `changes` job's
  `outputs:` block, and keep each declaration forwarding the identically-named
  classify output (`unraid: ${{ steps.classify.outputs.unraid }}`). A typo on
  either side reads as the empty string.
  `crates/labby/tests/ci_changed_paths.rs` and the classify step itself both
  fail the build on that mistake.
- Every declared `changes` output must be emitted by `changed_paths.py`, except
  the runtime-only `gate_key_drift`.
- `ci-gate` requires `changes` to conclude `success`. A skipped or cancelled
  `changes` job leaves every gate expression empty, which would skip every
  gated job and turn the whole run vacuously green.

Pinning the classifier also pins its path → category **mappings**, which lag
the same way key names do: a branch that routes a new directory into an
existing category gets a well-formed `false` from the base commit's classifier,
and the gated job skips for real — no empty string, nothing for the drift
reconciliation to notice. The classify step therefore re-runs the branch's own
`scripts/ci/changed_paths.py` over the **trusted changed-file list** and unions
the result: a `false` the branch raises to `true` is taken, and nothing else is.
A value it lowers, a key it invents, and the changed-file list itself are all
ignored, so a branch can broaden its own CI but never narrow it. Broadened keys
are annotated on the run. If the branch classifier cannot run, routing degrades
to the trusted classifier alone with a warning rather than failing.

`scripts/ci/changed_paths.py` is the only place the routing key list lives. The
fallback classifier used when the base commit predates that script emits no
keys at all and lets reconciliation force every gated key to `true`, so it is
not a second copy of the list.

Branch protection on `main` requires both `Repository Contract` and `ci-gate`.
The latter is the stable aggregate for branch-controlled CI jobs: heavy jobs
may skip when their category is false, while failed or cancelled dependencies
fail the aggregate. Native Windows workspace tests are required; the Palette
Windows job remains advisory.

Protected historical work products are enforced separately by
`.github/workflows/protected-docs.yml`. It runs on `pull_request_target`, checks
out only the trusted base revision, and queries the pull request file list
through the read-only GitHub token. Any change below `docs/sessions/` or
`docs/superpowers/` fails `Protected docs guard` unless a maintainer explicitly
applies the `protected-docs-approved` label. Label and unlabel events rerun the
guard. The guard is a separate required branch-protection context because it
must remain anchored to the base workflow rather than the pull request's
branch-controlled `ci.yml`.

If you add another normal CI job that must block merges, wire it into `ci-gate`
rather than adding another required context. Separate required contexts are
reserved for controls, like the protected-docs guard and repository contract,
that intentionally execute from a trusted workflow boundary.

The rmcp dependency is constrained by exact version and immutable Git revision
in Cargo metadata, so the ordinary manifest, lockfile, security, and conformance checks cover SDK
changes without a copied vendor tree or a separate vendor-approval workflow.

## CI Checks

Every push and pull request must pass `ci-gate`, which covers the following
jobs when their changed-path category is enabled:

| Check | Category | Command |
|-------|----------|---------|
| Unraid plugin checksums | `unraid` | `scripts/ci/unraid-plugin-checksums.sh` — fails if `unraid/labby.plg`'s companion-file `<MD5>` entities drift from `unraid/source/`. The `--tag`/`--tarball` form (checking `labbyVersion` and the release-tarball `<MD5>`) is a manual tool run when deliberately re-pointing `labbyVersion` at a new release — not a CI gate, since a freshly-built tarball's MD5 isn't reproducible run-to-run |
| Protected docs guard | separate required `pull_request_target` workflow | blocks `docs/sessions/**` and `docs/superpowers/**` changes unless a maintainer applies `protected-docs-approved` |
| Workflow lint | `workflow` | `actionlint` over `.github/workflows/` |
| Frontend build | `rust_compile`, `docs_check`, `web`, `docker`, or `release` | `./.github/actions/build-gateway-admin` (`pnpm install --frozen-lockfile && pnpm build` in `apps/gateway-admin`) |
| Gateway Admin browser tests | `web` | frozen install, pinned Playwright Chromium provisioning, and `pnpm test:browser`; explicitly aggregated by `ci-gate` |
| Compile | `rust_compile` | `cargo check --workspace --all-features` |
| MSRV | `rust_compile` | `cargo +1.97.1 check --workspace --all-features --all-targets --locked` |
| Feature slices | `rust_compile` | warm `labby` lib/bins at normal concurrency, then run `cargo check -p labby --no-default-features --features <slice> --all-targets --locked` for `gateway`, `gateway-host`, `integrated-gateway`, `fs`, and `skills` at the same concurrency so the heavy normal library is reused; gateway, fs, and skills retain focused runtime tests |
| Extracted crate slices | `rust_compile` | crate-specific `cargo check` commands for extracted runtime crates |
| Generated docs freshness | `docs_check` | `just docs-check` |
| Format | `rust_compile` | `cargo fmt --all -- --check` |
| Lint | `rust_compile` | warm `labby` lib/bins first (which warms normal gateway dependencies), lint extracted workspace all-targets, then run `cargo clippy -p labby --all-features --all-targets --locked -- -D warnings` at unchanged Cargo concurrency |
| Deny | `security` | `cargo deny check` |
| Palette renderer | `palette` | frozen install, lint, Vitest coverage, typecheck, and Vite build |
| Palette Tauri | `palette` | independent lockfile audit plus required Linux tests and an advisory native Windows build/test smoke |
| Rust coverage | `rust_test` | Required PR/push LCOV gate with project and critical auth/gateway/dispatch/config floors |
| Tests (Linux) | `rust_test` | warm normal `labby` lib/bins first, then `cargo nextest run --workspace --all-features --profile ci` on GitHub-hosted `ubuntu-24.04` |
| Tests (Linux fork PR fallback) | `rust_test` | same warm-up plus nextest run on GitHub-hosted `ubuntu-24.04` without repository secrets |
| Tests (Windows) | `rust_test` | same nextest run on GitHub-hosted `windows-latest`, including fork PRs; required by `ci-gate` |
| MCP conformance | `rust_test` or `workflow` | Labby's revision-pinned rmcp authenticated smoke, dated `2026-07-28` suites, and the checked MCP/OpenAI auth denominator in `conformance/auth-requirements.json` |
| MCP upstream drift | weekly/manual separate workflow | compares pinned MCP spec and rmcp commits, maps upstream changes to Labby code and required tests, and opens or updates one actionable issue |
| Release metadata contract | `release` | version and Rust toolchain lockstep only; release builds do not run in PR CI |
| Container source contract | `docker` | validates the Dockerfile and required source inputs without building an image |

Every distributable or deployable Labby binary must include the `skills`
feature. The Cargo feature graph makes `gateway` depend on `skills`, so the
default `gateway-host`, the sealed `integrated-gateway`, and `all` profiles all
include it. Featureless and non-gateway slices exist only to verify dependency
boundaries. Release binaries, the production container, and the Incus image
each run a packaged-artifact smoke that proves the Skills CLI surface exists.
The standalone Skills job runs the `skills::` test filter, covering shared
registry/provider behavior as well as MCP adapters without gateway support.

Clippy runs with `-D warnings` — zero warnings are permitted. This is enforced at the workspace lint layer. Feature-slice, Clippy, Linux test, and focused MCP regression jobs deliberately keep job-wide `CARGO_BUILD_JOBS` unset so cold native dependencies such as `aws-lc-sys` retain parallel builds. To avoid runner OOMs from concurrently compiling large normal libraries and their lib-test harnesses from a cold graph, those jobs first warm ordinary `labby`/gateway targets at normal concurrency and then run their all-target or test-harness pass at the same Cargo job count. The later phase reuses the heavy normal libraries while preserving target coverage and native build-script parallelism.

The frontend build is required because the Rust binary embeds the exported
Labby assets. It is a production build gate, not a TypeScript strictness gate:
`apps/gateway-admin/next.config.mjs` currently sets
`typescript.ignoreBuildErrors = true`. Run `pnpm test` in
`apps/gateway-admin` for the frontend unit and install-script test contract.

The required lifecycle-analysis job parses every shipped POSIX/Bash lifecycle
script with its declared shell, runs ShellCheck at warning severity, and runs
PSScriptAnalyzer 1.24.0 against the shipped Windows installer. Analyzer setup,
parse failures, warnings, and errors all fail the stable `ci-gate`.

MCP conformance details, exact reproducibility pins, and the strict extension
gap baseline are documented in
[MCP_CONFORMANCE.md](../surfaces/MCP_CONFORMANCE.md).

The advisory `MCP upstream drift` workflow watches both the MCP specification
repository and the latest rmcp release. Its pinned inputs live in
`conformance/upstream-baseline.json`; `scripts/ci/mcp_upstream_drift.py`
translates upstream file/release changes into the Labby modules and validation
commands that must be reviewed. It updates a stable issue rather than creating
notification spam. Never advance the baseline merely to silence the issue:
land the required code/tests and the baseline update together.

## CI Platform

- **Provider:** GitHub Actions
- **Manual runs:** `CI` supports `workflow_dispatch`
- **Scheduled runs:** `CI` runs weekly on Monday at 09:23 UTC to keep
  dependency/advisory visibility fresh even when no PR is active
- **Job split:**
  - `changes` classifies paths first and exports category booleans, forcing any gated key the trusted base-branch classifier cannot emit to `true`
  - Frontend assets build once when required, then Rust compile/lint/test jobs download the exported `apps/gateway-admin/out` artifact
  - Required fast jobs run only when their category is enabled on GitHub-hosted runners; `ci-gate` is the stable required check for branch protection
  - Native Windows workspace and Palette jobs use GitHub-hosted runners, bounded timeouts, and keyed Cargo caches; workspace tests block `ci-gate`, while Palette remains advisory
  - Heavy release work starts from an immutable stable-version tag while the
    matching GitHub release is still draft
  - Release Linux jobs use GitHub-hosted x86_64 runners; native macOS and Windows artifacts use GitHub-hosted runners

The pinned fleet policy and repository contract set `allow-arm64: true` for
Labby. This removes the former fleet-wide ARM64 token rejection while keeping
the shared workflows' default x86_64-only for callers that do not opt in. The
current release matrix remains the support matrix below until ARM64 jobs and
artifacts are added and verified.

## GitHub-hosted runners

All repository-defined Linux jobs use the GitHub-hosted `ubuntu-24.04` image.
Native Windows jobs use `windows-latest`. No repository-defined job selects a
self-hosted runner or a custom runner label.

Rust jobs use the repository `setup-rust-kache` composite in credentialless
GitHub Actions cache mode. Do not configure `KACHE_S3_ACCESS_KEY` or
`KACHE_S3_SECRET_KEY` as repository secrets: same-repository pull requests can
edit branch-controlled workflow files and therefore cannot safely coexist with
repository-level shared-cache credentials. Remove any legacy copies of those
secrets from repository settings. A future shared writer must use a separately
protected environment whose deployment-branch policy permits only `main`, plus
server-enforced least-privilege credentials; a client-side prefix is not an
authorization boundary.

The reusable fleet policy and repository contract are organization-managed
workflow calls. Their execution environment is owned by the central workflows
repository and is outside this repository's local runner selection.

## Build Matrix

| Platform | Target |
|----------|--------|
| Linux x86_64 | `x86_64-unknown-linux-gnu` |
| macOS arm64 | `aarch64-apple-darwin` |
| Windows x86_64 | `x86_64-pc-windows-msvc` |

macOS and Windows are supported platforms. Official macOS artifacts are built
on a native GitHub-hosted Apple Silicon runner. Official Windows artifacts are
built on native GitHub-hosted Windows runners using the MSVC target.
Cross-compilation may be useful experimentally, but it is not the release
support contract.

## Integration Tests

Live service integration tests are **excluded from CI**. They require real service instances and are run locally only.

```bash
# Local only — never runs in CI
just test-integration
```

Integration tests must be marked `#[ignore]` so `cargo nextest run` skips them without explicit opt-in.

## Release Process

1. Release Please prepares the version/changelog PR.
2. Merging that PR creates the stable `vX.Y.Z` tag plus a draft GitHub release.
3. The immutable tag triggers candidate work; no maintainer manually publishes
   the draft. Preflight requires stable SemVer, ancestry from `origin/main`, and
   exact Cargo/npm/MCP/release-manifest version lockstep.
4. Each platform archive is built, smoke-tested, and attested in its build job.
   The N-1 matrix verifies that exact archive attestation before extraction,
   checks the archive sidecar, and records an archive-to-extracted-binary digest
   binding. It then invokes a platform-owned adapter for Unix, Windows, macOS, Compose,
   Incus, and host-service deployment. Each adapter must install N-1 and seed
   real application-schema rows in registered OAuth clients, access security
   events, and upstream usage calls, plus representative files in gateway
   configuration and credentials, snippets, imported skills, and artifact
   state. It must verify every class, verify candidate provenance before
   activation, upgrade, perform authenticated work, restart and verify recovery,
   roll back, restart the rolled-back service, verify the same state remains
   readable, and repeat the authenticated action. A missing command or adapter
   is a hard failure. Compose qualification uses the production descriptor,
   digest-identifies both images, exercises every durable-state class and an
   authenticated catalog action, captures a state backup, and always tears down
   its isolated project after success or failure. The archive is the attestation subject; the extracted
   binary digest is the activation-integrity binding, not a claimed attestation.
5. The final gated job verifies archive checksums and creates one SPDX JSON SBOM
   for each archive, installer, and the exact tested container image. It records every
   subject digest in `release-manifest.json`, records every published checksum
   as an auxiliary subject, and attests the archives, checksums, SBOMs, and
   manifest.
6. Before activation/promotion, the workflow runs `gh attestation verify` with
   the exact repository, signer workflow, source ref, and hosted-runner policy.
   Offline consumers may pass a downloaded bundle and trusted root through the
   same GitHub CLI verification contract.
7. The tested image is published by digest and signed keylessly. If publication
   fails, the rollback transaction attempts deletion, `latest` restoration,
   Incus-pointer restoration, and restoration of the GitHub release to draft.
   It verifies each final state independently, emits one compound JSON record,
   and fails if any recovery step or final-state proof fails. Because npm and
   MCP versions are immutable, a failed transaction also records either
   published identity as `manual_reconciliation_required`; rollback can never
   claim success while one remains externally visible.
8. The Incus and MCP Registry workflows are reusable calls from the candidate
   graph. Incus publishes only immutable, version-namespaced candidate assets;
   the parent release transaction verifies that generation in place, records
   its checksummed `generation.json` alongside the versioned release, and moves
   only the rolling Git ref with a force-with-lease compare-and-swap. The
   recovery receipt retains the exact prior ref target, so rollback is another
   pointer-only leased CAS and never rewrites generation contents.
   They return validated 64-hex subject digests before the stable GitHub release
   becomes visible. npm publishes the immutable version under a version-specific
   candidate dist-tag; the `latest` consumer pointer is not advanced yet. No
   distribution workflow is triggered by `release.published`.
9. Only after every candidate qualification and publisher succeeds does
   `release.yml` promote the draft through the verified promotion helper. It
   advances and verifies npm's `latest` dist-tag only after that promotion.
   Promotion failure enters the same recovery path and retains an actionable
   record of immutable registry identities that cannot be deleted.
10. The aggregate reconciler runs immediately after Release completes and on a
   bounded schedule for eventual-consistency recovery. It paginates draft and
   published releases. A draft missing its manifest is itself a version-keyed
   incomplete record; manifest-bearing drafts are fully observed. Historical
   published releases from before the manifest contract remain excluded. It keeps each
   version's result independent so a newer complete release cannot hide an older
   incomplete one. It downloads each manifest and observes
   GitHub assets, npm version, GHCR digest, Incus asset digest, and the MCP v0.1
   version endpoint. The MCP publisher and observer hash the same canonical JSON
   object, so reconciliation fails closed unless the complete registry object,
   not merely its name and version, matches the published manifest digest. It
   also verifies the GitHub attestation identity for every
   manifest-declared archive, checksum, SBOM, installer, and the manifest
   itself. Missing, unexpected, unattested, or digest-mismatched subjects keep
   one `Release publication is incomplete` incident open and the run failed.

The npm and MCP registries do not support deleting an already-published
version. If one publisher succeeds and another fails, repair the discrepancy
and rerun the same immutable tag. Release assets are uploaded only when absent;
an existing byte-identical asset is reused and byte drift fails closed rather
than clobbering it. Publishers are idempotent and the aggregate
incident remains open until observations match the manifest. Never create a
replacement tag or bump the version merely to hide partial publication.

**Tag format:** `vX.Y.Z` — no other formats are accepted.

**Version policy:** single version across the entire workspace. `labby` and
`labby-apis` always share the same version number.

## Artifact Distribution

- **Surface:** GitHub Releases
- **Container surface:** GitHub Container Registry (`ghcr.io/dinglebear-ai/labby`)
- **Artifacts per release:** one binary archive per supported target (Linux x86_64, macOS arm64, and Windows x86_64)
- **Checksums:** every binary archive has a SHA-256 checksum file
- **SBOMs:** one identity-bound SPDX JSON document per archive and one for the
  exact tested container image
- **Manifest:** `release-manifest.json` binds every promoted subject name, size,
  and SHA-256 digest to its SBOM, and binds the exact GHCR digest to the
  container SBOM, for reconciliation
- **Package registries:** the `@dinglebear/labby` npm launcher and `server.json` MCP Registry metadata publish from the same validated version.

Before activating a downloaded archive, consumers run the repository helper so
the subject digest, source repository, signer workflow, tag ref, and hosted
runner identity are all enforced:

```bash
scripts/ci/verify-release-provenance.sh \
  --repo dinglebear-ai/labby \
  --workflow release.yml \
  --ref refs/tags/v1.14.1 \
  --artifact lab-x86_64-unknown-linux-gnu.tar.gz
```

For offline verification, download the attestation bundle and trusted root on a
connected trusted host, transfer them with the artifact, and add
`--bundle attestation.jsonl --trusted-root trusted_root.jsonl`. The same
repository/workflow/ref policy remains mandatory; offline mode never degrades
to checksum-only verification. Wrong signer, repository, ref, subject digest,
bundle, or trusted root is a hard failure before activation.

## MCP Registry DNS Key Rotation

The release workflow verifies `mcp-publisher` against the exact v1.8.0 GitHub
release asset SHA-256 before the `MCP_PRIVATE_KEY` secret enters the process.
Key rotation is a coordinated DNS and GitHub operation; never change only one
side or print the private key in a workflow log.

1. On a trusted host with OpenSSL 3, generate a fresh Ed25519 key:
   `openssl genpkey -algorithm Ed25519 -out key.pem`.
2. Derive the public value with
   `openssl pkey -in key.pem -pubout -outform DER | tail -c 32 | base64`.
3. Replace the TXT record at the **`dinglebear.ai` apex** with exactly one
   `v=MCPv1; k=ed25519; p=<public-key>` value. The registry does not use an
   `_mcp-*` selector, and the old record must be removed rather than retained.
4. After authoritative and public DNS both return only the new record, derive
   the private hex value with
   `openssl pkey -in key.pem -noout -text | grep -A3 'priv:' | tail -n +2 | tr -d ' :\n'`.
5. Replace the repository `MCP_PRIVATE_KEY` Actions secret using a no-echo
   channel, run `mcp-publisher login dns --domain dinglebear.ai --private-key "$MCP_PRIVATE_KEY"`, and verify an idempotent metadata publication.
6. Securely destroy the local plaintext key after the secret and DNS record
   have been verified; if any step fails, restore both prior DNS and secret
   together.

## Test Reports

CI uses the `ci` nextest profile in `.config/nextest.toml`. The test job
uploads `target/nextest/ci/junit.xml` as the `nextest-junit` artifact with
short retention so failed runs can be inspected without scraping logs.

## Cargo Deny Advisories

`deny.toml` keeps unmaintained advisory checks enabled. Any ignored advisory
must include a dependency-path comment and should be removed once the upstream
dependency path is gone. The weekly scheduled CI run keeps those exceptions
visible even if no pull request touches dependency policy.

## Size Policy

Binary size is tracked but not hard-gated in CI unless repo tooling enforces a monolith size limit. If a size gate is added, it runs in the fast check job.

## Frontend Tests

The shared `build-gateway-admin` action installs dependencies, verifies the
synced installer, runs `pnpm run test:unit`, runs `pnpm exec tsc --noEmit`, and
then runs `pnpm build`. This is the CI gate for the embedded gateway-admin
assets that are compiled into the `lab` binary. Keep TypeScript explicit here:
`next.config.mjs` intentionally ignores build-time TypeScript errors so asset
builds are not the type-safety boundary.

```bash
cd apps/gateway-admin
pnpm run test:unit
pnpm exec tsc --noEmit
pnpm test
pnpm test:acp
pnpm test:browser
```

## Non-Goals

- no telemetry pipeline
- no background analytics
- no phone-home behavior in any CI or release step
