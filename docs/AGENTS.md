# docs/ — Product Documentation Rules

This tree contains both canonical product documentation and historical/work-product material. Do not treat every Markdown file as current product truth.

## Canonical Product Docs

Use `docs/README.md` as the index. Current product behavior belongs in the service, surface, runtime, developer-contract, design, contract, guide, plugin, and generated-doc areas that it indexes.

`docs/generated/` is code-owned. Regenerate it with `just docs-generate`; never hand-edit generated artifacts.

## Historical / Non-Canonical Material

`docs/sessions/` and `docs/superpowers/` are protected historical/work-product trees: do not edit, move, retire, or link-audit them during a general documentation review. Explicitly approved PR changes require the maintainer-applied `protected-docs-approved` label. `docs/archive/` is historical, not current product truth.

`docs/references/` is an untracked research cache. `docs/plans/` contains engineering plans, not implemented guarantees. Neither overrides current code or canonical contracts; preserve historical evidence rather than silently rewriting it as present behavior.

Follow [Documentation Maintenance](dev/DOCUMENTATION.md) for source ownership, instruction aliases, local overrides, regeneration, and validation. Keep this directory's `AGENTS.md` canonical; `CLAUDE.md` and `GEMINI.md` remain relative symlinks to it.

Reports, old feature briefs, completed proposals, and other dated artifacts may explain history but are not automatically current contracts.

## Editing Rules

- verify claims against live code and generated catalogs
- prefer one canonical doc per concern; merge or retire redundant product docs
- use current `labby` names and `crates/labby-*` paths
- preserve intentional protocol identifiers such as `lab://...`, `ui://lab/...`, and `lab:admin`
- keep links relative and valid from the file that contains them
- if service/action metadata changes, regenerate docs in the same change
- document current behavior, not planned behavior, as implemented fact
