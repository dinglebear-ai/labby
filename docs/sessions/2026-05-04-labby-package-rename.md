---
date: 2026-05-04 06:15:58 EST
repo: git@github.com:jmagar/lab.git
branch: bd-work/mcp-gateway-review-remediation
head: 60939ce2
agent: Codex
session id: 88e8d4be-5916-447c-8c23-a788dfcb7a62
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/88e8d4be-5916-447c-8c23-a788dfcb7a62.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab  60939ce2 [bd-work/mcp-gateway-review-remediation]
pr: #40 Integrate service wave and CI updates https://github.com/jmagar/lab/pull/40
---

# Session: Resolve Cargo `lab` Package Ambiguity

## User Request

The session began with a `just dev-debug` failure:

```text
error: cannot specify features for packages outside of workspace
help: a workspace member with a similar name exists: `lab`
```

The user then asked what `lab@0.11.0` was, requested that this repo's package name be changed to `labby` without pinning, and clarified that the binary name should also become `labby`.

## Session Overview

- Diagnosed the `just dev-debug` failure as a Cargo package-name collision between this workspace's package formerly named `lab` and a crates.io package named `lab`.
- Confirmed the external `lab@0.11.0` came from the lockfile as a registry package, not from this repo.
- Renamed this repo's binary crate package and executable from `lab` to `labby`.
- Updated active build, install, Docker, deploy, TUI, and documentation references that pointed at the old package selector or binary artifact.
- Verified both normal all-features package checking and the original nightly Cranelift debug build path with unpinned `-p labby`.

## Sequence of Events

1. Read the `superpowers:systematic-debugging` skill and inspected `Justfile`, workspace manifests, and memory for prior package ambiguity context.
2. Found `Justfile` used stale package selectors like `lab@0.12.1` while the workspace crate version was `0.12.2`.
3. Tried switching package selectors to plain `lab`, which exposed the deeper issue: Cargo reported `lab` was ambiguous between `lab@0.11.0` and `lab@0.12.2`.
4. Inspected `Cargo.lock` and confirmed `lab@0.11.0` was a registry dependency, while this repo's package was the path dependency under `crates/lab`.
5. After user direction, renamed this repo's package and binary to `labby`, then replaced active package selectors and binary paths.
6. Ran Cargo metadata, package ID, `cargo check`, and the original nightly Cranelift build command to verify the rename.
7. Captured this session in this markdown file using `vibin:save-to-md`.

## Key Findings

- `Cargo.lock` retained a crates.io package named `lab` at version `0.11.0`; that is why `-p lab` was ambiguous before the rename.
- `crates/lab/Cargo.toml:2` now declares the package as `labby`.
- `crates/lab/Cargo.toml:15` now declares the binary target as `labby`.
- `crates/lab/src/cli.rs:115` now sets the Clap command name to `labby`.
- `Justfile:16`, `Justfile:20`, and `Justfile:60` now use unpinned `labby` package selectors.
- `config/Dockerfile:53`, `config/Dockerfile:72`, `config/Dockerfile:93`, and `config/Dockerfile:102` now build, copy, and run `labby`.

## Technical Decisions

- Renamed the local package instead of continuing to pin `lab@<version>` so build commands can use stable unpinned selectors.
- Renamed the binary as well because the user explicitly allowed changing the executable name to `labby`.
- Kept the source directory `crates/lab/` unchanged; Cargo package identity and binary target were sufficient to remove the package collision.
- Used `RUSTC_WRAPPER=` for verification commands after `sccache` failed in the sandbox with `Operation not permitted`.
- Did not run the full `just dev-debug` recipe because it also installs the binary and restarts the Docker dev stack; the Cargo build line that originally failed was verified directly.

## Files Modified

- `crates/lab/Cargo.toml` - renamed package and binary target to `labby`.
- `crates/lab/src/cli.rs` - changed CLI command display name to `labby`.
- `Cargo.lock` - refreshed the local package entry from `lab` to `labby`.
- `Justfile` - changed docs and debug package selectors to unpinned `labby`; updated install paths to `bin/labby` and `target/.../labby`.
- `config/Dockerfile` and `config/Dockerfile.fast` - changed build selectors, copied binary paths, and runtime entrypoints to `labby`.
- `README.md`, `crates/lab/README.md`, `docs/dev/ERRORS.md`, `docs/runtime/DEPLOY_SERVICE.md`, and `plugins/lab/skills/lab-service-onboarding/references/contracts.md` - updated active examples and references from `lab` to `labby` where they referred to package or binary paths.
- `crates/lab/src/config.rs`, `crates/lab/src/node/update.rs`, `crates/lab/src/dispatch/deploy/*`, `crates/lab/src/tui/*`, `crates/lab/src/dispatch/gateway/manager.rs`, and `crates/lab/tests/deploy_runner.rs` - updated default deployment, update, TUI, gateway, and test binary paths to `/usr/local/bin/labby` or `target/.../labby`.

## Commands Executed

- `rg -n "dev-debug|name = \"lab\"|version = \"0\\.12\\.1\"|members|package" Justfile Cargo.toml crates/lab/Cargo.toml` - found stale `lab@0.12.1` selectors and the local package metadata.
- `cargo metadata --no-deps --format-version 1` - confirmed workspace package metadata and versions.
- `cargo pkgid -p lab@0.11.0` - showed `registry+https://github.com/rust-lang/crates.io-index#lab@0.11.0`.
- `cargo pkgid -p lab@0.12.2` - showed the local path package before the rename.
- `rg -n "lab@0\\.[0-9.]+|--package lab\\b|-p lab\\b|target/(debug|release)/lab\\b|bin/lab\\b|/usr/local/bin/lab\\b|name = \"lab\""` - found active stale selectors and binary paths to update.
- `cargo pkgid -p labby` - after the rename, resolved to `path+file:///home/jmagar/workspace/lab/crates/lab#labby@0.12.2`.
- `cargo check -p labby --all-features` - verified the renamed package with all features.
- `cargo +nightly build -p labby --all-features` with the Cranelift `RUSTFLAGS` - verified the original failing build path.

## Errors Encountered

- `cargo +nightly build -p 'lab@0.12.1' --all-features` failed because `lab@0.12.1` no longer matched this workspace package.
- `cargo +nightly build -p lab --all-features` failed because Cargo saw both `lab@0.11.0` from crates.io and the local `lab@0.12.2`.
- `cargo tree -i lab@0.11.0` initially failed because `sccache` could not run in the sandbox: `Operation not permitted`; rerunning Cargo commands with `RUSTC_WRAPPER=` avoided the wrapper.
- `cargo pkgid -p labby` briefly failed before Cargo refreshed the lockfile; a subsequent `cargo check -p labby --all-features` updated lock metadata and compiled `labby`.

## Behavior Changes (Before/After)

| Area | Before | After |
| --- | --- | --- |
| Package selector | Build recipes had to pin `lab@<version>` or risk ambiguity. | Active recipes can use unpinned `labby`. |
| Binary artifact | Builds produced `target/debug/lab` and `target/release/lab`. | Builds produce `target/debug/labby` and `target/release/labby`. |
| Installed CLI path | Local and container install paths used `bin/lab` or `/usr/local/bin/lab`. | Updated active install/runtime paths use `bin/labby` or `/usr/local/bin/labby`. |
| CLI command name | Clap displayed the command as `lab`. | Clap displays the command as `labby`. |
| External crates.io `lab` package | Collided with the local package name. | Still present in `Cargo.lock`, but no longer collides with the local package. |

## Verification Evidence

| command | expected | actual | status |
| --- | --- | --- | --- |
| `RUSTC_WRAPPER= cargo pkgid -p labby` | Resolves to the local workspace package. | `path+file:///home/jmagar/workspace/lab/crates/lab#labby@0.12.2` | PASS |
| `RUSTC_WRAPPER= cargo check -p labby --all-features` | All-features package check succeeds. | Finished `dev` profile in 40.32s. | PASS |
| `RUSTC_WRAPPER= RUSTFLAGS="-C link-arg=-fuse-ld=mold -Z codegen-backend=cranelift" cargo +nightly build -p labby --all-features` | Original `dev-debug` Cargo build line succeeds without package ambiguity. | Finished `dev` profile in 1m 35s. | PASS |
| `rg -n "lab@0\\.[0-9.]+|--package lab\\b|-p lab\\b|target/(debug|release)/lab\\b|bin/lab\\b|/usr/local/bin/lab\\b|name = \"lab\""` over active build/source surfaces | No stale active package selectors or binary artifact paths. | No matches in the checked active surfaces. | PASS |

## Risks and Rollback

- Runtime scripts, services, or external hosts that still execute `lab` directly will need to be updated or given a compatibility wrapper outside this repo.
- Historical docs under `docs/sessions/` and `docs/superpowers/plans/` still contain old commands as historical records; they were not bulk-rewritten.
- Rollback path: restore `crates/lab/Cargo.toml` package and binary names to `lab`, restore build/runtime paths to `lab`, regenerate `Cargo.lock`, and return to explicit package-id selectors to avoid ambiguity.

## Decisions Not Taken

- Did not keep package name `lab` with a pinned version because the user explicitly asked not to pin.
- Did not rename the `crates/lab/` directory because Cargo does not require the directory name to match the package name, and renaming it would create a much larger path churn.
- Did not run Docker restart or install steps from `just dev-debug`; only the failing Cargo build step was necessary to verify the package ambiguity fix.

## References

- Active PR: #40 `Integrate service wave and CI updates` - https://github.com/jmagar/lab/pull/40
- `superpowers:systematic-debugging` skill was used for the initial root-cause investigation.
- `vibin:save-to-md` skill was used to create this session record.

## Open Questions

- Whether external automation should keep a temporary `lab` compatibility symlink or wrapper is not decided in this session.
- Whether historical documentation should be bulk-updated from `lab` to `labby` is not decided; many references are historical session notes or old plans.

## Next Steps

- Started but not completed: none.
- Follow-on: update any host-level service units, shell wrappers, MCP configs, or remote deploy targets outside this repo that still invoke `lab`.
- Follow-on: if this note should be committed, remember that `docs/sessions/` may be ignored and may require `git add -f docs/sessions/2026-05-04-labby-package-rename.md`.
