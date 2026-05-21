# Stale Docs Audit

Date: 2026-05-01

## Scope

This audit checked the repo docs against the current checkout across five
lanes:

- service inventory, env vars, coverage docs, upstream references, and generated catalogs
- runtime architecture docs for config, node runtime, deploy, gateway, OAuth, stash, and marketplace
- link/index/navigation hygiene
- developer, testing, CI, and release docs
- CLI, MCP, API, serialization, and generated help surfaces

## Changes Applied

- Fixed CI/CD docs to match the current GitHub Actions surface: frontend build,
  Rust check/fmt/clippy/deny/nextest, Linux release targets, and GitHub release
  notes.
- Updated CLI docs from `device`/`/v1/device/*` to `nodes`/`/v1/nodes/*` and
  refreshed stale command examples.
- Updated MCP docs to remove the obsolete `lab` meta-tool claim, document
  actual destructive confirmation behavior, and remove unsupported
  action-level resource URIs.
- Updated config and runtime docs for `[node].controller`,
  `log_retention_days`, CWD `.env`, durable node logs, and node-to-controller
  wording.
- Updated deploy-service docs from placeholder `NoopRunner` status to the live
  SSH runner path and noted `lab deploy monitor`.
- Fixed broken links caused by the move to `docs/design/SERIALIZATION.md`.
- Added directory indexes for `docs/coverage/`, `docs/upstream-api/`,
  `docs/generated/`, `docs/features/`, and `docs/design/`.
- Added explicit ACP Registry coverage/upstream notes and a NotebookLM upstream
  contract note.
- Updated qBittorrent coverage for username/password login fallback and SID
  shortcut behavior.
- Updated service docs for always-on exposed services and Dozzle inventory.
- Added `dozzle` to the `lab-apis` `all` feature list so the code matches the
  documented all-services contract.

## Generated Catalog Status

The generated MCP catalog was checked by an agent against fresh compiled help
and matched. This pass also removed the reported OpenACP `-y, --yes` drift from
`docs/generated/cli-help.md`. Refreshing `cli-help.md` still needs a checked-in
generator or a scripted recursive help capture.

## Remaining Follow-Ups

- Add a checked-in `just docs-generate` or script that refreshes
  `docs/generated/cli-help.md`, `docs/generated/mcp-help.md`, and
  `docs/generated/mcp-help.json` deterministically.
- Decide whether `docs/CHANGELOG.md` should be deleted, archived, or converted
  into a pointer to root `CHANGELOG.md`.
- Consider expanding `docs/GATEWAY.md`, `docs/MARKETPLACE.md`, and
  `docs/OAUTH.md` into fully regenerated action/route inventories. This pass
  corrected high-confidence stale claims but did not rewrite those larger docs
  end to end.
- Decide whether `acp_registry` should remain SDK-only or be onboarded as a
  full CLI/MCP/API service.
