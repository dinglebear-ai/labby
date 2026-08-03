---
title: "Rust Build Setup"
created: "2026-07-30"
updated: "2026-08-02"
doc_type: "guide"
status: "active"
owner: "lab"
audience:
  - "contributors"
  - "agents"
scope: "service"
source_of_truth: false
upstream_refs:
  - "https://github.com/dinglebear-ai/soma/blob/main/docs/RUST.md"
last_reviewed: "2026-05-15"
---

# Rust Build Setup

This repo follows the build conventions of the rmcp server family.
The canonical reference is [soma/docs/RUST.md](https://github.com/dinglebear-ai/soma/blob/main/docs/RUST.md).

## System prerequisites

- Rust stable ≥ 1.86 (`rustup update stable`)
- `clang` and `mold` for fast Linux builds: `apt install clang mold`
- `just` command runner (optional): `cargo install just`
- `kache` (optional compiler cache; installed through the managed mise config)

## Global Cargo config

Build performance depends on `~/.cargo/config.toml` on the developer's machine.
See [soma/docs/RUST.md](https://github.com/dinglebear-ai/soma/blob/main/docs/RUST.md)
for the expected config (mold linker, profile settings, Cranelift backend).

## Local `.cargo/config.toml`

This repo's `.cargo/config.toml` has one intentional override:

```toml
[build]
incremental = false
```

**Why:** compiler caches cannot safely reuse incremental artifacts. The managed
host config already disables incremental compilation, and this repo repeats the
invariant so builds remain cache-friendly on other developer machines.

Developers without sccache take no penalty from this override — Rust simply
recompiles changed crates in full rather than using incremental fragments.

The host Cargo configuration owns compiler caching. This repository deliberately
does not set `rustc-wrapper`, and building never installs or refreshes a live
binary as a compiler side effect. Use `just build-release`, `just install`, or
`just host-sync` when an installed binary should change.

## Kache troubleshooting

On managed hosts, `~/.cargo/config.toml` sets `rustc-wrapper = "kache"`. Kache
is content-addressed and fail-open: a cache problem can make a build slow without
making it fail, so a green build is not proof that the cache is healthy.

**Symptom — "poisoning":** builds produce stale or wrong artifacts (code you
deleted still seems present, link errors that don't match source, nondeterministic
failures). The cache is returning artifacts that don't match the current inputs.

Inspect and repair it directly:

```bash
kache doctor
kache doctor --verify --repair
kache why-miss <crate>
kache stats
```

**Bypass for a single build** (e.g. release/CI-parity verification you don't want
to trust the cache for):

```bash
KACHE_DISABLED=1 cargo build --workspace --all-features
# or bypass every configured wrapper:
RUSTC_WRAPPER="" cargo build --workspace --all-features
```
