---
title: "Documentation Maintenance"
created: "2026-09-27"
updated: "2026-09-27"
---

# Documentation Maintenance

[AGENTS.md](../../AGENTS.md) is the canonical contributor instruction file.
[docs/README.md](../README.md) is the product-documentation index. They serve
different purposes: instructions govern repository work; current implementation
and code-owned catalogs establish implemented product behavior.

## Sources of truth

| Concern | Authority |
| --- | --- |
| Contributor rules | Root and nearest nested `AGENTS.md` |
| Workspace members, features, toolchain | Cargo manifests and `rust-toolchain.toml` |
| Developer commands | `Justfile` and package scripts |
| Service/action/surface inventory | Current registration/dispatch code and `docs/generated/` |
| CLI commands and flags | Clap graph and generated CLI help |
| Runtime configuration and auth | Implementation plus the owning runtime/contract doc |
| CI routing, checks, release targets | `.github/`, `scripts/ci/`, and the CI/CD contract |
| Operator/task-oriented explanations | The current topic docs indexed by `docs/README.md` |

Avoid duplicating a command or service inventory in instructions. Link to the
owning generated catalog or manifest when an exact list is needed. Keep
accepted decisions distinct from proposed designs and historical evidence.

## Instruction topology

Every maintained instruction scope has exactly one authored source:

```text
AGENTS.md             regular, nonempty Markdown file
CLAUDE.md -> AGENTS.md relative symlink
GEMINI.md -> AGENTS.md relative symlink
```

Edit AGENTS.md. Preserve existing specialized rules when changing topology;
renaming a file is not permission to discard its invariants. Read root and
nested instructions together. References in shared guidance should name
AGENTS.md even though the compatibility aliases continue to resolve.

For a new scope, write its AGENTS.md first, then run these commands from that
directory. They intentionally fail rather than overwrite an existing file:

```bash
ln -s AGENTS.md CLAUDE.md
ln -s AGENTS.md GEMINI.md
```

Git should store AGENTS.md as a regular file and each alias with mode 120000.
A copied Markdown alias, absolute target, reversed topology, empty source, or
orphan alias is a validation failure. Enable actual symlink support in the
checkout rather than maintaining independent platform-specific copies.

## Private per-checkout instructions

The official [Codex instruction discovery guide](https://developers.openai.com/codex/guides/agents-md/)
names `AGENTS.override.md`; the official [Claude Code memory guide](https://code.claude.com/docs/en/memory)
names `CLAUDE.local.md`. Neither `AGENTS.local.md` nor `CLAUDE.md.local` is the
standard filename for this setup. Legacy aliases may remain ignored for local
compatibility, but must not become independent authored sources.

Use this optional local layout, separate from the tracked instruction topology:

```text
AGENTS.override.md                 regular, private, Git-ignored source
CLAUDE.local.md -> AGENTS.override.md  relative, Git-ignored symlink
```

Codex chooses at most one instruction file per directory and checks the override
before AGENTS.md. The override therefore must explicitly direct Codex to read
the shared root AGENTS.md and the relevant nested instructions before work.
Claude loads CLAUDE.local.md alongside the shared CLAUDE.md alias. An optional
`@AGENTS.md` import is understood by Claude; do not assume Codex implements
Claude's import syntax. Include the plain-language read instruction too:

```markdown
# Local contributor context

Before working, read AGENTS.md at this worktree's Git root and the nearest
nested AGENTS.md for the area being changed. These local notes supplement,
not replace, the shared project rules.

@AGENTS.md

## Local environment

Add only private workstation and deployment context here. Store credentials
in the configured credential source, never in instruction files.
```

After creating AGENTS.override.md, run `ln -s AGENTS.override.md CLAUDE.local.md`
from the checkout root. Inspect and preserve an existing file before migrating;
do not blindly overwrite local notes. Verify both names with `git check-ignore`
and ensure neither appears in `git ls-files`. Ignored files are worktree-local,
so a new worktree needs its own explicitly provisioned local setup.

Keep host addresses, service observations, private notes, and all credentials
out of tracked instructions. Shared rules must remain portable between checkouts.


## Review an existing topic

Identify the owning source files, manifests, generated catalogs, and tests.
Compare every changed claim with that evidence. Separate compiled capability
from platform gating, runtime registration, caller authorization, and current
deployment. A feature present in source is not proof that an installed server
runs it or exposes it to a particular route.

Update the owning topic first, then summaries and indexes that point to it.
Preserve relative links and heading anchors. Do not replace durable contracts
with session-specific status, dates, benchmark measurements, or machine paths.
When a document contains dated metadata, update it only after reviewing its
content, not as a substitute for review.

The npm launcher's README is an intentional exception to deduplication:
`packages/labby-mcp/scripts/sync-readme.js` copies the root README before
packaging. Change the root and run the sync script; do not edit the copy
independently.

## Historical and generated boundaries

`docs/sessions/` and `docs/superpowers/` are protected historical/work-product
trees. A normal documentation audit must not edit, move, retire, or link-audit
them. Explicitly approved PR changes require the maintainer-applied
`protected-docs-approved` label under the protected-docs workflow.

`docs/archive/` is historical. `docs/references/` is an untracked external
reference cache. Plans and feature packets are not implemented fact; maintained
ones may still receive link checks, but should point back to current contracts
when completed. Preserve their historical evidence rather than silently
rewriting the implementation record.

`docs/generated/` is code-owned. Use:

```bash
just docs-generate
just docs-check
```

Do not hand-edit generated catalogs. Review their diffs for accidental host,
feature, or platform leakage before committing them.

## Automated gates

The repository-contract job also requires `title`, `created`, and `updated`
frontmatter on maintained topic documents. Its AGENTS-first adapter keeps all
other checks from the immutable fleet implementation and replaces only the
legacy reverse-direction symlink rule with strict Git-index validation. See
[CI/CD](../runtime/CICD.md) for the adapter and required aggregate contract.

`scripts/check-product-docs.py` inventories tracked and nonignored new paths
through Git. It checks current product links, stale naming, duplicate prose,
service-index coverage, install/auth guidance, CLI examples, and instruction
topology. It rejects private instruction filenames that are tracked or not
ignored, including force-added local overrides. Canonical AGENTS.md files participate in product-doc validation;
symlink aliases are not duplicate documents. Ignored worktrees, private
configuration, build outputs, and protected historical trees are not audit
inputs. Failure to obtain the Git inventory is an error, not a reason to fall
back to scanning the entire machine.

`scripts/check-doc-links.py` validates maintained Markdown links and heading
fragments, including canonical instruction files. The regression suites in
`scripts/ci/test_product_docs.py` and `scripts/ci/test_doc_links.py` protect
these boundaries. Add a fixture whenever changing a checker, not just a prose
rule describing its intended behavior.

The full `just docs-check` recipe also checks code-generated freshness,
Depot control-plane documentation, and shipped skill CLI examples. Finish with
`git diff --check` and confirm the protected trees were not changed. A passing
link checker proves link integrity, not that every product claim is correct;
manual source comparison remains required.
