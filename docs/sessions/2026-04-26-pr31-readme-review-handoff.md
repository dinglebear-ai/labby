# PR 31 README And Review Handoff

Date: 2026-04-26 18:45 EDT

## Repository State

- Repository: `/home/jmagar/workspace/lab`
- Branch: `feat/product-readme-and-marketplace-surface`
- Upstream: `origin/feat/product-readme-and-marketplace-surface`
- PR: https://github.com/jmagar/lab/pull/31
- PR title: `Expand product and marketplace surface docs`
- Base branch: `main`
- Current HEAD at capture: `5cf4c60c ci: build gateway-admin before cargo so include_dir! finds out/`
- Working tree at capture: clean

## What Changed

The README/product-surface branch expanded the root README into the canonical public entrypoint for Lab. It now describes the major product capabilities, including marketplace browsing, Claude Code and Codex marketplace support, cherry-picking/installing skills/agents/commands/MCP servers, MCP Registry search/install, MCP registry aggregation, ACP Registry agent search, deployment to devices, stash workspaces, fork/patch/update flows, upstream MCP proxying, auth/OAuth, the web UI chat, and composable feature selection.

Follow-up PR review fixes landed in `03c23ae0 fix: address PR review feedback`:

- Preserved explicit root `[tool_search] enabled = false` during legacy upstream `tool_search` migration.
- Preserved string channel paths from marketplace manifests instead of replacing them with array indexes or map keys.
- Advertised synthetic `tool_search` / `tool_invoke` tools when root tool-search mode is enabled even with no configured upstream gateways.
- Removed stale upstream-level tool-search validation attribution from gateway config validation.

CI follow-up landed in `9f3acae4 fix: update rustls-webpki advisory`:

- Updated `rustls-webpki` from `0.103.10` to `0.103.13` in `Cargo.lock`.
- This cleared `RUSTSEC-2026-0104` for `cargo deny check`.

There is also a later pushed commit visible at capture:

- `5cf4c60c ci: build gateway-admin before cargo so include_dir! finds out/`

## Review Threads

The repo-local `plugins/skills/gh-address-comments` scripts were used for PR #31.

Five review threads were addressed and resolved:

- `PRRT_kwDOR8nC1M59sLQm` — explicit root `tool_search` disable during migration.
- `PRRT_kwDOR8nC1M59sLQp` — preserve channel path values for string channel entries.
- `PRRT_kwDOR8nC1M59sK00` — channel array string entries routed through inline config.
- `PRRT_kwDOR8nC1M59sK0u` — root-enabled tool search with zero upstreams should still expose synthetic tools.
- `PRRT_kwDOR8nC1M59sK0w` — remove stale `InvalidToolSearch*` match arms from upstream validation.

GitHub GraphQL verification showed all five review threads as `isResolved: true`.

Local review beads were closed:

- `lab-py2h`
- `lab-ew36`
- `lab-8z35`
- `lab-5936`
- `lab-wyzf`

Note: `fetch_comments.py` hit a script bug when sorting reviews with `submittedAt = null`, so final review-thread verification used direct `gh api graphql` output instead of the cached helper snapshot.

## Verification Run

Local verification performed:

- `cargo test -p lab@0.11.1 --all-features tool_search` — passed.
- `cargo test -p lab@0.11.1 --all-features components_from_manifest` — passed.
- `cargo check -p lab@0.11.1 --all-features` — passed.
- `cargo fmt --all --check` — passed.
- `git diff --check` — passed.
- `cargo deny check` — initially failed on `RUSTSEC-2026-0104`, then passed after the `rustls-webpki` lockfile update.

`cargo deny check` still emits existing duplicate/license/advisory-not-detected warnings, but the final summary was:

```text
advisories ok, bans ok, licenses ok, sources ok
```

## PR Status At Capture

`gh pr view 31` showed:

- State: open.
- Merge state: `UNSTABLE`.
- Passing: Check, Format, Clippy, Cargo Deny, GitGuardian.
- Neutral: Cubic.
- Pending: Test (ubuntu-latest), Test (windows-latest), CodeRabbit.

## Open Questions

- Whether the latest pending GitHub test jobs complete successfully after `5cf4c60c`.
- Whether the `fetch_comments.py` null `submittedAt` sorting bug should be fixed in `plugins/skills/gh-address-comments/scripts/fetch_comments.py`.
