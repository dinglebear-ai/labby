---
title: "Development Workflow"
created: "2026-09-27"
updated: "2026-09-27"
---

# Development Workflow

This is the checkout-to-validation guide for contributors. Read the repository
[AGENTS.md](../../AGENTS.md) and the nearest nested AGENTS.md before editing.
[Architecture](../ARCH.md) owns boundaries; [Testing](./TESTING.md) owns test
policy; [CI/CD](../runtime/CICD.md) owns the release and CI contract.

## Establish the checkout and preserve work

Run these commands from the selected checkout, not from a different project's
shell working directory:

```bash
git rev-parse --show-toplevel
git status --short --branch
git remote -v
git worktree list
git diff --stat
git diff --cached --stat
```

Confirm the destination is the intended Labby repository before publishing.
Preserve staged, unstaged, untracked, and unpublished work. An existing dirty
checkout is not permission to reset it. For parallel changes, create an isolated
worktree from the intended base rather than switching another agent's branch.
Never sweep sibling worktrees into a commit or force-add ignored host state.

## Toolchain and packages

The executable contracts are [rust-toolchain.toml](../../rust-toolchain.toml),
[Cargo.toml](../../Cargo.toml), [Justfile](../../Justfile), and each package's
manifest and lockfile. Check these files rather than assuming a globally
installed tool has the required version.

The root Rust workspace has 13 members. The verification toolkit under
[tools/verification](../../tools/verification/README.md) is a separate workspace
with a separate lockfile. The native desktop shell also has its own Cargo
manifest; root workspace tests do not replace either set of checks.

The Gateway Admin manifest currently requires Node 22.x and pins pnpm 9.15.9.
Install its locked dependencies from that package before frontend work:

```bash
rustup show active-toolchain
just --list
just rust-toolchain-sync
pnpm --dir apps/web install --frozen-lockfile
```

Use the existing host compiler cache when healthy. See [Technology](../TECH.md)
for cache diagnostics; do not delete a shared cache or stop another agent's
build as a routine workaround.

## Select the owning layer

| Change | Start here |
| --- | --- |
| Shared action vocabulary, SSRF primitives | `crates/labby-primitives` |
| Pure setup/doctor SDK contracts | `crates/labby-apis` |
| Auth, OAuth, credential/session lifecycle | `crates/labby-auth` |
| Upstream connections, catalog, routing, gateway Code Mode host | `crates/labby-gateway` |
| Host-neutral JavaScript execution and snippets | `crates/labby-codemode` |
| Reusable browser bridge | `crates/labby-browser` |
| OpenAPI parsing and hardened outbound execution | `crates/labby-openapi` |
| Shared runtime, Artifact, task, and authority contracts | `crates/labby-runtime` |
| Product operations and thin CLI/MCP/API adapters | `crates/labby/src` |
| Static operator UI / native shell | `apps/web` / `apps/tauri` |

Do not add a built-in service for an external capability that can be an upstream
MCP server. Follow [Service Onboarding](./SERVICE_ONBOARDING.md) for a genuine
Labby-owned lifecycle.

## Validate the changed behavior

Run focused tests first, then the relevant repository gates. Compilation alone
is not behavioral verification. The root recipes are:

```bash
just check
just test
just lint
just docs-check
just rustdoc-check
```

For example, a gateway runtime change needs its focused regression tests plus
`cargo nextest run -p labby-gateway --all-features`. Cross-surface metadata,
errors, authorization, or exposure changes need checks for every affected
adapter, not only the CLI or only MCP. Platform-specific behavior requires the
matching platform lane; macOS success is not Linux/Windows acceptance.

For Gateway Admin changes:

```bash
pnpm --dir apps/web lint
pnpm --dir apps/web test
pnpm --dir apps/web test:browser
just web-build
```

The static export is consumed by the Rust host. A development page loading is
not evidence that the embedded production export works. Desktop and verification
changes use their respective nested instructions and package-specific gates.

## Documentation-only changes

Use [Documentation Maintenance](./DOCUMENTATION.md). Fast feedback without a
Rust build is available through:

```bash
python3 scripts/check-doc-links.py
python3 scripts/check-product-docs.py
python3 -m unittest discover -s scripts/ci -p 'test_doc_links.py'
python3 -m unittest discover -s scripts/ci -p 'test_product_docs.py'
```

These checks do not replace `just docs-check`, which also compiles/runs the
all-feature documentation generator and checks generated freshness and other
contracts. Regenerate code-owned catalogs with `just docs-generate` when their
source contracts change; never patch generated output to hide a mismatch.

## Local runtime versus installed deployment

Builds and tests do not update an installed gateway. Use a distinct absolute
`LABBY_HOME` and explicitly selected loopback listener for an isolated preview.
Consult `labby serve --help` and [Runtime Configuration](../runtime/CONFIG.md)
for the current flags and authority rules. Never silently point a preview at
production state, disable auth behind a reverse proxy, or substitute local
state when an explicit remote target fails.

Installation, service restarts, releases, and production deployment are separate
operations with separate authorization and verification. Local host paths and
credentials belong in ignored local configuration, not shared instructions.

## Before publishing

Review the complete staged diff and `git diff --check`. Verify any requested
pre-existing changes are included without credentials, ignored state, build
products, or unrelated worktrees. Fetch and reconcile the authorized destination
without force-pushing. After publication, compare local HEAD with the actual
remote branch and inspect final working-tree status. Report the commit, checks
that passed or failed, and any remaining limitations separately from deployment
or live acceptance.
