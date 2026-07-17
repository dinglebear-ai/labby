---
date: 2026-07-17 18:59:29 EDT
repo: git@github.com:jmagar/labby.git
branch: agent/add-server-mcp-app
head: b34a8828
working directory: /home/jmagar/.codex/worktrees/0752ff24-4e7c-45c4-ab5b-12d18b43fa6c/lab
worktree: /home/jmagar/.codex/worktrees/0752ff24-4e7c-45c4-ab5b-12d18b43fa6c/lab
pr: "#250 feat(mcp): add responsive server onboarding app — https://github.com/jmagar/labby/pull/250"
beads: lab-5mbcv, lab-5mbcv.1, lab-5mbcv.2, lab-5mbcv.3, lab-5mbcv.4, lab-5mbcv.5, lab-5mbcv.6, lab-5mbcv.7, lab-5mbcv.8, lab-5mbcv.9
---

# MCP gateway apps, review, and live deployment

## User Request

Add a mobile-friendly MCP App for adding upstream servers, expose all intended Labby app tools to Claude, add a Gateway Status app for connected upstreams, run the requested Lavra reviews, address every finding, merge the work, build the current binary, and sync it to Incus.

## Session Overview

The session delivered and merged the Add Server and Gateway Status MCP Apps, fixed dynamic tool-list notifications, completed a scoped Lavra review with nine tracked findings, and merged all remediation in PR #254. The merged `main` binary and web assets were deployed to Incus, and live MCP discovery confirmed four app tools—including `gateway_status`—plus the versioned app resources.

## Sequence of Events

1. Designed and implemented the responsive Add Server app, including local-command and remote-URL onboarding, connection testing, optional resource/prompt exposure, Aurora styling, and mobile layout; merged PR #250.
2. Investigated why Claude initially showed only Code Mode and Add Server. Updated downstream tool-list change detection so newly available app tools trigger `tools/list_changed`; merged PR #253.
3. Implemented the read-only Gateway Status app and its synthetic `gateway_status` tool/resource contract; merged PR #252.
4. Ran the requested Lavra review scoped to the two new MCP Apps. Thirty review lenses across three teams produced nine validated findings, each recorded as a child bead under `lab-5mbcv`.
5. Fixed all findings, added executable browser-script behavior tests, documented discovery gates, added Node 22 to Rust CI jobs, and merged PR #254.
6. Diagnosed the missing live `gateway_status` tool as a stale Incus binary, rebuilt from merged `origin/main`, synced the binary and web export, and verified the public endpoint with `mcporter`.

## Key Findings

- The live Incus binary—not the gateway catalog—was stale. The host build contained `gateway_status`; the deployed `/usr/local/bin/labby` did not until the final sync.
- A successful MCP initialization can expose zero tools, resources, and prompts. `GatewayRuntimeView.connected` now carries authoritative connection state independently of capability counts (`crates/labby-gateway/src/gateway/types.rs:169`, `crates/labby-gateway/src/gateway/projection.rs:580`).
- Embedded app sizing must report document height only. Width feedback and positive height offsets can create host-dependent resize loops.
- Gateway Status had two asynchronous writers: launch hydration and manual refresh. Generation ordering now ensures the newest snapshot wins (`crates/labby/src/mcp/assets/gateway_status_app.html:49`, `crates/labby/src/mcp/assets/gateway_status_app.html:112`).
- Conditional MCP App discovery depends on admin scope, gateway manager/registry availability, route policy, and `gateway.list`; the operator contract is now documented (`docs/services/GATEWAY.md:200`, `docs/surfaces/MCP.md:183`).

## Technical Decisions

- Kept both apps as synthetic, narrowly scoped MCP tools instead of expanding the gateway dispatch tool surface.
- Used `GatewayRuntimeView.connected` as the source of truth rather than inferring connectivity from capability counts.
- Preserved backend structured error envelopes and stale-data timestamps so refresh failures remain actionable.
- Retained a small browser-side command tokenizer but specified and tested exact behavior for empty quoted arguments, literal backslashes, Windows paths, escaped spaces and quotes, and trailing backslashes.
- Added executable Node-based tests to the existing Rust test harness and pinned Node 22 in relevant CI jobs so JavaScript behavior is tested without introducing a frontend test framework.

## Files Changed

| status | path | previous path | purpose | evidence |
|---|---|---|---|---|
| modified | `.github/workflows/ci.yml` | — | Install pinned Node 22 for executable MCP App behavior tests | PR #254 |
| modified | `crates/labby-gateway/src/gateway/manager/tests/lifecycle.rs` | — | Align gateway lifecycle expectations with onboarding behavior | PR #250 |
| modified | `crates/labby-gateway/src/gateway/manager/tests/views.rs` | — | Verify zero-capability connected runtime views | PR #254 |
| modified | `crates/labby-gateway/src/gateway/manager/views.rs` | — | Supply gateway runtime data used by onboarding | PR #250 |
| modified | `crates/labby-gateway/src/gateway/projection.rs` | — | Project authoritative connected state | PR #254 |
| modified | `crates/labby-gateway/src/gateway/types.rs` | — | Add `GatewayRuntimeView.connected` | PR #254 |
| modified | `crates/labby-gateway/src/upstream/pool/tools.rs` | — | Include app-tool availability in list-change snapshots | PR #253 |
| modified | `crates/labby/src/app_assets.rs` | — | Register and version embedded app assets | PRs #250 and #252 |
| modified | `crates/labby/src/mcp/CLAUDE.md` | — | Record MCP App implementation guidance | PR #250 |
| created | `crates/labby/src/mcp/assets/add_server_app.html` | — | Responsive Add Server MCP App | PR #250 |
| created | `crates/labby/src/mcp/assets/gateway_status_app.html` | — | Gateway Status MCP App | PR #252 |
| modified | `crates/labby/src/mcp/assets/server_logs_app.html` | — | Keep shared app behavior consistent | PR #250 |
| modified | `crates/labby/src/mcp/call_tool.rs` | — | Dispatch synthetic app tools and structured results | PRs #250 and #252 |
| modified | `crates/labby/src/mcp/catalog.rs` | — | Advertise Add Server and Gateway Status under their visibility predicates | PRs #250, #252, and #253 |
| modified | `crates/labby/src/mcp/handlers_resources.rs` | — | Serve app resources and add executable behavior/discovery tests | PRs #250, #252, and #254 |
| modified | `crates/labby/src/mcp/handlers_tools.rs` | — | Register and invoke synthetic app tools | PRs #250 and #252 |
| modified | `crates/labby/src/mcp/handlers_tools/tests.rs` | — | Cover discovery, authorization, dispatch, and list-change behavior | PRs #250, #252, and #253 |
| modified | `docs/services/GATEWAY.md` | — | Document app tools, visibility gates, and stale-binary troubleshooting | PRs #250 and #254 |
| modified | `docs/surfaces/MCP.md` | — | Document MCP app tool/resource contracts | PRs #250 and #254 |
| created | `docs/sessions/2026-07-17-mcp-ui-review-gateway-status-deployment.md` | — | Preserve this session | This commit |

## Beads Activity

| bead | title | actions | final status | why it mattered |
|---|---|---|---|---|
| `lab-5mbcv` | Review and harden shipped Add Server and Gateway Status MCP Apps | Created, tracked review scope, closed after verification | closed | Parent record for the scoped Lavra review |
| `lab-5mbcv.1` | Stop MCP App intrinsic resize feedback loops | Created from P1 finding, fixed, verified, closed | closed | Prevented host-dependent clipping and runaway resizing |
| `lab-5mbcv.2` | Report successful zero-capability MCP handshakes accurately | Created from P2 finding, fixed, verified, closed | closed | Removed false disconnected states |
| `lab-5mbcv.3` | Preserve structured Gateway Status errors | Created, annotated with learned/pattern comments, fixed, closed | closed | Retained actionable backend failures |
| `lab-5mbcv.4` | Make Gateway Status hydration newest-wins | Created, annotated with learned/pattern comments, fixed, closed | closed | Prevented stale launch output from replacing newer refreshes |
| `lab-5mbcv.5` | Parse Add Server command arguments without corruption | Created, annotated with learned/pattern comments, fixed, closed | closed | Preserved exact local command argv |
| `lab-5mbcv.6` | Complete Add Server teardown cleanup | Created, annotated with learned/pattern comments, fixed, closed | closed | Removed retained listeners and stale controls |
| `lab-5mbcv.7` | Document Gateway Status discovery prerequisites | Created, annotated with learned/pattern comments, fixed, closed | closed | Made missing-tool diagnosis operationally clear |
| `lab-5mbcv.8` | Align MCP App warning semantics and sentence-case copy | Created, fixed, verified, closed | closed | Improved semantic styling and freshness wording |
| `lab-5mbcv.9` | Make Gateway Status app behavior maintainable and executable-testable | Created, fixed, verified, closed | closed | Replaced source-marker-only confidence with executable behavior checks |

## Repository Maintenance

- **Plans:** `docs/plans/complete/mcp-streamable-http-oauth-proxy.md` was already archived. `docs/plans/fleet-ws-plan-lab-n07n.md` was left in place because its completion state was not established by this session.
- **Beads:** Live `bd show` output confirmed the parent and all nine child beads were already closed with verification reasons; no tracker mutation or follow-up bead was required.
- **Worktrees and branches:** `git worktree list --porcelain`, `git branch -vv`, remote branches, and merge ancestry were inspected. The active worktree and canonical `main` worktree contain unrelated user changes, while squash-merged feature branches are not ancestors of `origin/main`; no worktree or branch was deleted.
- **Stale docs:** The two directly relevant docs were corrected in PR #254. No broader stale-doc rewrite was justified by the scoped work.
- **Publish isolation:** This note was prepared in a temporary docs-only worktree created from fresh `origin/main`, preserving all unrelated dirt.

## Tools and Skills Used

- **Lavra review skill and reviewer agents:** Applied the requested scoped review workflow across 30 lenses in three teams; nine validated findings were tracked and remediated.
- **Shell and file tools:** Used Git, Cargo, Just, pnpm, Node, `rg`, `actionlint`, systemd/Incus commands, and path-limited patching for implementation, diagnostics, builds, and verification.
- **GitHub CLI:** Inspected checks, created and merged PRs #250, #252, #253, and #254, and verified merge commits and file sets.
- **Beads CLI:** Created and closed the review parent and nine child findings, with diagnostic comments on recurring failure patterns.
- **mcporter:** Queried the deployed MCP endpoint, listed live tools/resources, and called `gateway_status` directly.
- **Labby Incus tooling:** Synced the merged release binary and web export, restarted the system service, and checked the public readiness URL.
- **Save-to-md skill:** Performed the maintenance audit and published only this generated session artifact. A temporary worktree initially triggered mise's untrusted-config guard for shimmed commands; absolute tool paths avoided mutating trust state.

## Commands Executed

| command | result |
|---|---|
| `cargo test -p labby --all-features --lib mcp::handlers_resources::tests::` | 41 focused MCP resource/app tests passed |
| `cargo nextest run -p labby-gateway` | 540 passed; 3 skipped |
| `cargo clippy -p labby --all-features --all-targets -- -D warnings` | Passed with no warnings |
| `just docs-check` | Passed; 15 generated artifacts were fresh |
| `actionlint .github/workflows/ci.yml` | Passed |
| `pnpm install --frozen-lockfile && just web-build` | Rebuilt the embedded web export successfully |
| `just build-release` | Built the all-features release binary from merged `main` |
| `target/release/labby incus sync --binary target/release/labby --web-assets-dir apps/gateway-admin/out --check-url https://mcp.dinglebear.ai/ready` | Deployed binary/assets and passed readiness check |
| `mcporter list lab-prod --json` | Reported `server_logs`, `codemode`, `add_server`, and `gateway_status` |
| `mcporter call lab-prod.gateway_status ...` | Returned 54 upstreams, 52 connected, and 0 warnings |

## Errors Encountered

- Claude did not show Gateway Status because Incus was running a stale binary without the `gateway_status` string. Rebuilding from merge commit `d9694563` and syncing to Incus resolved discovery.
- The first self-hosted Linux CI run failed with `mise ERROR No version is set for shim: node`. PR #254 explicitly installs pinned Node 22 in all Rust-test jobs.
- The web export dependencies were not ready in the review worktree. `pnpm install --frozen-lockfile` restored the locked dependencies before `just web-build`.
- During session-note publishing, shimmed commands in the temporary worktree hit mise's untrusted `.mise.toml` guard. Absolute paths to the already-installed tools avoided changing global trust configuration.

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| Claude tool discovery | New app tools could remain absent until an unrelated catalog change or reconnect | App availability contributes to downstream `tools/list_changed`; reconnect exposes all eligible tools |
| Add Server status | Valid zero-capability servers appeared disconnected | Successful initialization is connected even with zero capabilities |
| Add Server commands | Empty args and literal backslashes could be corrupted | Tested parsing preserves supported argv edge cases |
| Embedded sizing | Width/height feedback could collapse or grow the iframe | Apps request document height only and converge cleanly |
| Gateway Status hydration | Late launch data could overwrite a newer refresh | One-time hydration and generation ordering preserve newest data |
| Gateway Status errors | Structured backend messages became a generic invalid snapshot | Backend messages and stale timestamps remain visible |
| Live deployment | Incus exposed no `gateway_status` tool | Production exposes `gateway_status` and its versioned resource |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| Focused MCP App tests | All app behavior tests pass | 41 passed | pass |
| Gateway regression suite | No gateway regressions | 540 passed, 3 skipped | pass |
| All-target clippy | No warnings | Clean | pass |
| Docs validation | No generated-doc drift | 15 artifacts fresh | pass |
| Workflow lint | Valid GitHub Actions syntax | Clean | pass |
| Release build | All-features release binary | Built from `d9694563` | pass |
| Incus sync/readiness | New binary active and public endpoint ready | PID 68166; `labby 1.5.0`; readiness JSON returned `ready` | pass |
| Live MCP list | Four app tools including Gateway Status | `server_logs`, `codemode`, `add_server`, `gateway_status` | pass |
| Live Gateway Status call | Structured upstream snapshot | 54 total, 52 connected, 0 warnings | pass |

## Risks and Rollback

- The app tools are conditionally advertised. A client without `lab:admin`, a disabled route, denied `gateway.list`, or an unavailable gateway manager/registry will still omit them by design.
- Claude caches MCP discovery for an existing connector session; reconnecting or restarting Claude may be necessary after deployment.
- Rollback is to deploy the prior known-good Labby binary and matching `apps/gateway-admin/out` export through the same `labby incus sync` command.

## Decisions Not Taken

- Did not expose one MCP tool per upstream or gateway action; the synthetic app tools remain focused entry points over existing dispatch semantics.
- Did not add a frontend testing framework; the existing Rust harness executes focused JavaScript behavior with Node.
- Did not delete squash-merged branches or dirty worktrees during maintenance because ancestry and ownership evidence did not make deletion unambiguously safe.

## References

- [PR #250: Add responsive server onboarding app](https://github.com/jmagar/labby/pull/250)
- [PR #252: Add gateway upstream status app](https://github.com/jmagar/labby/pull/252)
- [PR #253: Notify clients when app tools appear](https://github.com/jmagar/labby/pull/253)
- [PR #254: Harden gateway apps after review](https://github.com/jmagar/labby/pull/254)
- Public readiness endpoint: `https://mcp.dinglebear.ai/ready`

## Next Steps

- Reconnect the Labby connector or restart Claude if its existing session still shows the pre-deployment tool list.
- No unfinished implementation or review findings remain from this session.
- Treat the open release PR #251 as normal release automation; it was not part of this scoped session.
