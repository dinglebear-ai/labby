# Review Scope

## Target

The entire Lab repository at commit `e6d761f91466905b435b253497b5d4077882fba8`, including current `origin/main` plus the pending Code Mode MCP UI panel change. The review covers all 11,090 tracked files. Ignored build output, dependency caches, Git internals, and machine-local runtime state are excluded.

## Files

- All Rust workspace code and manifests under `crates/`, `Cargo.toml`, and `Cargo.lock`.
- All Gateway Admin React/Next.js code, tests, assets, and package metadata under `apps/` and the root JavaScript package files.
- All integration and contract tests under `tests/` and crate/app-local test modules.
- All operational scripts, configuration, container, Unraid, plugin, and packaging surfaces under `scripts/`, `config/`, `unraid/`, `plugins/`, and `packages/`.
- All GitHub Actions, release automation, security tooling, and repository policy files under `.github/` and the repository root.
- All source-of-truth documentation under `docs/`, `openwiki/`, and root documentation files, including accuracy checks against implementation.
- Every remaining tracked file returned by `git ls-files` at the review commit.

Tracked-file distribution: `docs` 9,922; `apps` 530; `crates` 513; `plugins` 28; `scripts` 13; `.github` 13; `unraid` 10; `packages` 10; `config` 9; `openwiki` 5; `tests` 4; and 33 root or single-file entries.

## Flags

- Security Focus: no
- Performance Critical: no
- Strict Mode: no
- Framework: auto-detected (Rust 2024/Tokio/Axum/rmcp plus Next.js 16/React 19/TypeScript)

## Review Phases

1. Code Quality & Architecture
2. Security & Performance
3. Testing & Documentation
4. Best Practices & Standards
5. Consolidated Report

The user explicitly pre-approved continuing through both phase checkpoints without pausing.
