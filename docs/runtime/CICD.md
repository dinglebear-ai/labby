# CI/CD

Last updated: 2026-06-27

This document is the authoritative contract for CI, release, and artifact delivery in `lab`. All pipeline implementations must conform to this spec.

## CI Path Routing

`ci.yml` starts with a `changes` job that runs `scripts/ci/changed_paths.py`.
That classifier maps the changed file list into stable routing categories:
`docs`, `docs_check`, `workflow`, `rust_compile`, `rust_test`, `web`, `palette`,
`docker`, `security`, and `release`. Scheduled and manual runs enable every
category so periodic/manual validation stays broad.

Branch protection should require the stable aggregate `ci-gate` check. The
heavy jobs below may be skipped when their category is false; `ci-gate` treats
`success` and intentionally `skipped` jobs as acceptable, and fails on failed or
cancelled dependencies.

## CI Checks

Every push and pull request must pass `ci-gate`, which covers the following
jobs when their changed-path category is enabled:

| Check | Category | Command |
|-------|----------|---------|
| Unraid plugin checksums | always | `scripts/ci/unraid-plugin-checksums.sh` — fails if `unraid/labby.plg`'s companion-file `<MD5>` entities drift from `unraid/source/`. The `--tag`/`--tarball` form (checking `labbyVersion` and the release-tarball `<MD5>`) is a manual tool run when deliberately re-pointing `labbyVersion` at a new release — not a CI gate, since a freshly-built tarball's MD5 isn't reproducible run-to-run |
| Workflow lint | `workflow` | `actionlint` over `.github/workflows/` |
| Frontend build | `rust_compile`, `docs_check`, `web`, `docker`, or `release` | `./.github/actions/build-gateway-admin` (`pnpm install --frozen-lockfile && pnpm build` in `apps/gateway-admin`) |
| Gateway Admin browser tests | `web` | frozen install, pinned Playwright Chromium provisioning, and `pnpm test:browser`; explicitly aggregated by `ci-gate` |
| Compile | `rust_compile` | `cargo check --workspace --all-features` |
| Feature slices | `rust_compile` | `cargo check -p labby --no-default-features --features <slice>` |
| Extracted crate slices | `rust_compile` | crate-specific `cargo check` commands for extracted runtime crates |
| Generated docs freshness | `docs_check` | `just docs-check` |
| Format | `rust_compile` | `cargo fmt --all -- --check` |
| Lint | `rust_compile` | `cargo clippy --workspace --all-features -- -D warnings` |
| Deny | `security` | `cargo deny check` |
| Palette renderer | `palette` | frozen install, lint, Vitest coverage, typecheck, and Vite build |
| Palette Tauri | `palette` | independent lockfile audit plus Linux tests and native Windows build/test smoke |
| Rust coverage | `rust_test` | LCOV trend artifact with project and critical auth/gateway/dispatch/config floors |
| Tests (Linux) | `rust_test` | `cargo nextest run --workspace --all-features --profile ci` on the self-hosted `linux-ci` runner for trusted events |
| Tests (Linux fork PR fallback) | `rust_test` | same nextest run on `ubuntu-latest` for fork PRs |
| Tests (Windows) | `rust_test` | same nextest run on the self-hosted `windows-ci` Windows runner, with fork PRs excluded from self-hosted runners |
| Release smoke | `release` | `cargo build --workspace --all-features --release`; Windows release smoke still skips PRs via the matrix |
| Container smoke | `docker` | Docker build using `config/Dockerfile` |

Clippy runs with `-D warnings` — zero warnings are permitted. This is enforced at the workspace lint layer.

The frontend build is required because the Rust binary embeds the exported
Labby assets. It is a production build gate, not a TypeScript strictness gate:
`apps/gateway-admin/next.config.mjs` currently sets
`typescript.ignoreBuildErrors = true`. Run `pnpm test` in
`apps/gateway-admin` for the frontend unit/ACP test contract.

## CI Platform

- **Provider:** GitHub Actions
- **Manual runs:** `CI` and `Release` both support `workflow_dispatch`
- **Scheduled runs:** `CI` runs weekly on Monday at 09:23 UTC to keep
  dependency/advisory visibility fresh even when no PR is active
- **Job split:**
  - `changes` classifies paths first and exports category booleans
  - Frontend assets build once when required, then Rust compile/lint/test jobs download the exported `apps/gateway-admin/out` artifact
  - Heavy jobs run only when their category is enabled; `ci-gate` is the stable required check for branch protection
  - Release builds on `vX.Y.Z` tags only
  - Container image publishing and GitHub Release publishing after successful tag builds

## Linux Self-hosted Runner

The Linux full test job runs on a self-hosted runner with labels `self-hosted`
and `linux-ci` for trusted events.

- Fork PRs are still validated on `ubuntu-latest` via `test-fork`.
- Runner setup and containerized registration are documented in
  [Actions runner setup](./ACTIONS_RUNNER.md).

## Build Matrix

| Platform | Target |
|----------|--------|
| Linux x86_64 | `x86_64-unknown-linux-gnu` |
| Windows x86_64 | `x86_64-pc-windows-msvc` |

Windows is a supported platform. Official Windows release artifacts are built
on native GitHub-hosted Windows runners using the MSVC target. Linux-to-Windows
GNU cross-compilation may be useful experimentally, but it is not the release
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
2. Merging that PR creates the stable `vX.Y.Z` tag plus a private draft GitHub release and triggers release CI. Explicit tag creation is required because GitHub otherwise defers tags for draft releases.
3. Preflight requires strict stable SemVer, ancestry from `origin/main`, and exact Cargo/npm/MCP/release-manifest version lockstep.
4. Binary, Incus, and container candidates are built and smoke-tested as private workflow artifacts.
5. The final gated job verifies checksums, emits an SPDX SBOM and GitHub provenance attestations, then publishes the exact tested image by digest and signs it keylessly with Cosign.
6. The immutable image tag and compatibility `latest` tag advance together; failure deletes the new version and restores the previous `latest` digest.
7. The final job reuses the Release Please draft (or creates one for a manually pushed valid tag), uploads the verified assets, and makes it public last. Rollback deletes only releases and image versions created by that run; a pre-existing published release is never mutated.

The npm and MCP registries do not support deleting an already-published
version. If publication reaches one registry and then fails, rerun the same tag
after correcting the failure: the release job checks each registry first,
skips the version that already exists, republishes only the missing surface,
and makes the draft GitHub release public only after both registry versions are
present. Never create a replacement tag or bump the version to recover a
partially published release.

**Tag format:** `vX.Y.Z` — no other formats are accepted.

**Version policy:** single version across the entire workspace. `lab` and `lab-apis` always share the same version number.

## Artifact Distribution

- **Surface:** GitHub Releases
- **Container surface:** GitHub Container Registry (`ghcr.io/jmagar/lab`)
- **Artifacts per release:** one binary archive per supported target (Linux x86_64, Windows x86_64; aarch64 dropped deliberately — rquickjs-sys does not cross-compile and no fleet host is ARM)
- **Checksums:** every binary archive has a SHA-256 checksum file
- **Package registries:** the `labby-mcp` npm launcher and `server.json` MCP Registry metadata publish from the same validated version.

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
