---
title: "Rustdoc"
created: "2026-08-18"
updated: "2026-08-18"
---

# Rustdoc

This document is the source of truth for Labby's Rust API-documentation build and verification contract.

## Goals

The Rustdoc pipeline must:

- document every workspace library and binary-only package;
- include all Cargo features;
- render private items so maintainers can navigate implementation structure as well as public APIs;
- cover secondary binaries and examples that Cargo omits from a package's default documentation target;
- fail on broken or private intra-doc links, invalid Rustdoc HTML/code blocks, bare URLs, redundant explicit links, unescaped backticks, and missing crate-level documentation;
- run workspace doctests;
- publish browsable HTML from CI;
- report missing public API prose separately without forcing unrelated changes to repair the entire historical backlog.

## Commands

Build the complete HTML documentation:

```bash
just rustdoc
```

Build the same documentation and run all workspace doctests:

```bash
just rustdoc-check
```

Audit missing public API prose without failing the build:

```bash
just rustdoc-audit
```

Use `rustdoc-check` before completing changes that touch public Rust APIs, module/crate structure, Rustdoc, examples, binaries, or symbols referenced by intra-doc links.

## Target Coverage

Cargo's default workspace documentation pass covers workspace libraries plus packages whose primary/default target is a binary. It does not reliably cover every secondary binary or example when the same package also contains a library.

Labby therefore uses two output trees:

1. `target/doc/` contains the canonical workspace/default-target documentation.
2. `target/rustdoc-extra/doc/` contains the `stdio-mcp-fixture` integration-test binary and Labby's Rust examples.

The split is intentional. The `labby` package has both a library and a same-named binary; Cargo cannot publish both without an output collision at `labby/index.html` (Cargo issue #6313). The CLI launcher itself is six lines and delegates directly to `labby::run()`, so the canonical library Rustdoc is the product API documentation for that executable. CI compile-checks the launcher normally instead of publishing a duplicate page.

The secondary Rustdoc pass targets the non-colliding fixture and all examples. Cargo's `--examples` selector keeps example coverage current as examples are added.

## Strict Correctness Policy

Workspace Rustdoc lints are configured in the root `Cargo.toml`. The fleet contract retains the legacy Rust-table spelling of `missing_crate_level_docs`, while the canonical `rustdoc::missing_crate_level_docs` lint is also enabled. The strict build denies:

- missing crate-level documentation;
- broken intra-doc links;
- links from public documentation to private items;
- invalid Rust code blocks and code-block attributes;
- invalid HTML;
- bare URLs;
- redundant explicit links;
- unescaped backticks.

`just rustdoc` additionally sets `RUSTDOCFLAGS=-D warnings`, so Rustdoc warnings introduced by a change fail the build rather than silently accumulating.

If documentation needs to mention a private implementation detail, use code formatting such as `private_helper` rather than an intra-doc link that cannot resolve for public readers.

## Missing Public API Prose

`missing_docs` remains non-blocking at the workspace compiler level because the product crate has substantial historical public-API documentation debt. A temporary strict inventory during the 2026-08-18 documentation audit found 853 missing public items in the `labby` product library.

That backlog should be reduced deliberately rather than filled with low-value comments written only to satisfy a lint. `just rustdoc-audit` uses `--force-warn missing_docs` so it reports the backlog even though the workspace compiler policy allows it.

When touching a public API:

- add or improve meaningful Rustdoc for the touched contract;
- include field/variant documentation when the type is intended for external or cross-crate use;
- use intra-doc links for stable public symbols;
- keep examples executable as doctests when practical;
- avoid restating the identifier name without adding behavioral, ownership, safety, or contract information.

A future cleanup can ratchet `missing_docs` to `deny` crate-by-crate once each crate's existing backlog reaches zero.

## CI

The dedicated `Rustdoc` CI job runs `just rustdoc-check` whenever Rust compilation or workflow validation is selected by the changed-path classifier. The stable CI gate requires that job to succeed.

CI uploads both documentation trees as the `rustdoc-html` artifact:

- `target/doc/`
- `target/rustdoc-extra/doc/`

The Rustdoc job intentionally does not depend on the built Next.js admin bundle. Rustdoc describes Rust APIs and the web-embedding contract remains valid even when build-time web assets are empty during this lane.

## Related Docs

- [Testing](./TESTING.md)
- [Technology and Rust build](../TECH.md)
- [Conventions](../CONVENTIONS.md)
- [CI/CD](../runtime/CICD.md)
