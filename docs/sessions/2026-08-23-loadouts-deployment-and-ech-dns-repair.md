---
date: 2026-08-23 09:26:01 EDT
repo: git@github.com:dinglebear-ai/labby.git
branch: main
head: daf9caa488d5e3de0b236a7984ef550bfbfe6031
working directory: /home/jmagar/workspace/labby
worktree: /home/jmagar/workspace/labby
pr: "#481 Improve skills operator diagnostics and catalog browsing; #484 Fix stale Gateway Admin clients across deployments"
beads: lab-pmcvh
---

# Loadouts deployment and split-DNS ECH repair

## User Request

Create a fresh worktree, systematically debug the broken Loadouts page, fix and review the code through a PR, merge and deploy it, then diagnose and resolve the ECH fallback failure. The later request expanded the DNS repair to every affected split-DNS hostname and asked for durable documentation.

## Session Overview

- Reproduced the Loadouts failure, corrected static Next.js deployment identity handling, addressed every review finding, merged PR #484 through PR #481, and deployed exact `main` commit `daf9caa488d5e3de0b236a7984ef550bfbfe6031`.
- Diagnosed `ERR_ECH_FALLBACK_CERTIFICATE_INVALID` as a split-DNS metadata mismatch and repaired both Technitium resolvers.
- Expanded hostname-specific DNS records into zone-wide wildcard/apex coverage while correcting exact-name exceptions.
- Created homelab maintenance record `docs/maintenance/technitium/2026-08-23-split-dns-ech-and-labby-recovery.md` in the homelab repository.

## Sequence of Events

1. Created isolated Loadouts debugging and exact-main deployment worktrees.
2. Traced stale static navigation behavior to Next.js `deploymentId` semantics that require response-header coordination absent from Labby's Rust static server.
3. Implemented `generateBuildId`, portable revision resolution, artifact validation, and browser coverage; review found two issues and the revised implementation passed re-review.
4. Stabilized two unrelated-but-blocking CI fixtures, merged PR #484 into #481, merged #481 to `main`, built exact `main`, and deployed frontend assets plus the Labby binary.
5. Diagnosed intermittent Chrome ECH failure across Tailscale's two Technitium resolvers and corrected both.
6. Audited the whole `dinglebear.ai` and `tootie.tv` split-DNS zones, added wildcard/apex HTTPS records, corrected exact-node shadowing, and verified direct DNS, synthetic DNS, public controls, and fresh Chromium.

## Key Findings

- `deploymentId` was the wrong mechanism for a pure static export because Labby does not inject `x-nextjs-deployment-id`; `generateBuildId` preserves a comparable build ID in static Flight payloads.
- Revision lookup must prefer a validated explicit build value, guard Git lookup, and retain a stable per-build fallback when Git is unavailable.
- Internal private `A` answers combined with forwarded Cloudflare HTTPS records containing `ech=` caused Chrome to negotiate public ECH against private SWAG.
- Both Tailscale global resolvers must be changed and tested; otherwise resolver selection makes the problem intermittent.
- DNS wildcards do not supply a missing record type after an exact node exists, requiring paired exact `A` and HTTPS records for exceptions.

## Technical Decisions

- Used Next.js static build IDs rather than adding response-header synthesis to every Rust/external static-serving path.
- Kept Cloudflare public ECH records unchanged and overrode HTTPS/SVCB metadata only in internal Technitium zones.
- Added zone-wide apex and wildcard records for routine new hostnames; left exact-record automation as tracked bead `lab-pmcvh`.
- Used official Technitium APIs after AXFR was denied; backed up binary zone files before mutation.

## Files Changed

| status | path | previous path | purpose | evidence |
|---|---|---|---|---|
| modified | `apps/gateway-admin/next.config.mjs` | — | Static build identity and portable revision resolution | PR #484 |
| created | `apps/gateway-admin/lib/build-id.test.ts` | — | Build-ID resolver tests | PR #484 |
| created | `apps/gateway-admin/scripts/check-static-build-id.mjs` | — | Validate exported Loadouts build identity | PR #484 |
| modified | `apps/gateway-admin/lib/browser/gateway-detail.browser.test.ts` | — | Same-build and stale-build navigation coverage | PR #484 |
| modified | `apps/gateway-admin/package.json` | — | Run static build-ID validation after build | PR #484 |
| modified | `apps/gateway-admin/app/(admin)/page.tsx`, `traces/page.tsx`, `usage/page.tsx`, `app/globals.css` | — | PR #481 operator UI and responsive behavior | PR #481 file manifest |
| modified | `apps/gateway-admin/components/console/console-sidebar.tsx`, `loadouts/loadout-form-dialog.tsx`, `loadouts/loadouts-page-content.tsx`, `skills/skills-page-content.tsx`, `tools/tool-browser.tsx` | — | Operator navigation, Loadouts, Skills, and tool-browser behavior | PR #481 file manifest |
| created/modified | `apps/gateway-admin/components/loadouts/loadout-form-dialog.test.tsx`, `components/skills/skills-page-content.test.tsx`, `components/tools/tool-browser.test.tsx` | — | Frontend regression coverage | PR #481 file manifest |
| created/modified | `apps/gateway-admin/lib/api/gateway-client.ts`, `gateway-client.test.ts`, `metrics-client.real.test.ts`, `skills-model.ts`, `skills-model.test.ts` | — | Gateway/Skills models and stable time fixture | PR #481 file manifest |
| created/modified | `apps/gateway-admin/lib/hooks/use-gateways.ts`, `use-gateways.test.ts` | — | Targeted gateway queries | PR #481 file manifest |
| modified | `crates/labby-codemode/src/discovery.rs`, `tests_discovery.rs` | — | Discovery behavior and tests | PR #481 file manifest |
| modified | `crates/labby-gateway/src/gateway/catalog.rs`, `config.rs`, `config_tests.rs`, `dispatch.rs`, `dispatch_tests.rs`, `manager/views.rs`, `runtime.rs` | — | Targeted snapshots, loadout-aware routes, and gateway fixture correction | PR #481 file manifest |
| modified | `crates/labby-gateway/src/upstream/pool.rs`, `pool/health.rs`, `pool/skills.rs`, `pool/skills_list.rs`, `pool/skills_tests.rs` | — | Upstream diagnostics and Skills behavior | PR #481 file manifest |
| modified | `crates/labby-runtime/src/gateway_config.rs`, `skills.rs`, `skills/frontmatter.rs`, `skills/manifest.rs` | — | Runtime route identity and Skills parsing/manifest behavior | PR #481 file manifest |
| modified | `crates/labby/src/api/router.rs`, `api/state.rs`, `cli/serve.rs`, `config.rs`, `mcp/call_tool.rs` | — | Product API/config/runtime wiring | PR #481 file manifest |
| modified | `docs/generated/action-catalog.json`, `action-catalog.md`, `mcp-help.json`, `mcp-help.md`, `openapi.json` | — | Regenerated product catalogs | PR #481 file manifest |
| created | `docs/sessions/2026-08-23-loadouts-deployment-and-ech-dns-repair.md` | — | This session artifact | current save-to-md workflow |
| created | `/home/jmagar/workspace/homelab/docs/maintenance/technitium/2026-08-23-split-dns-ech-and-labby-recovery.md` | — | Curated live homelab maintenance history | maintenance workflow |
| modified | `/home/jmagar/workspace/homelab/docs/maintenance/index.md` | — | Generated maintenance index | `refresh-maintenance-index.sh` |
| modified | Technitium binary zones for `dinglebear.ai` and `tootie.tv` on Squirts and Dookie | — | Internal ECH-free HTTPS and exact paired records | Technitium API plus `dig` verification |

## Beads Activity

| id | title | actions | final status | why it mattered |
|---|---|---|---|---|
| `lab-pmcvh` | Automate exact split-DNS A and HTTPS record pairs | created | open | Tracks the remaining durable provisioning workflow for exact exceptions and new zones |

## Repository Maintenance

- **Plans:** `skills-over-mcp-compat` remains active and `usage-metrics-first-class` retains an explicit unresolved policy decision; neither was moved to `docs/plans/complete/`.
- **Beads:** Existing tracker data was read; `lab-pmcvh` was created for the only known session follow-up. No completed bead was closed because the automation has not been implemented.
- **Worktrees/branches:** All registered worktrees and branches were inspected. Dirty worktrees and unmerged branches were preserved. The clean merged `debug-loadouts-page` and detached deployment worktrees were left in place because the user originally requested work in that worktree and their cleanup was not required to publish documentation.
- **Stale docs:** No Labby product doc was contradicted by the runtime repair. The live operational history was added to the homelab maintenance tree and its generated index refreshed.
- **Transcript:** The injected latest Claude transcript was inspected and identified as an unrelated August 10 Aurora repository-status session, so it was not used as evidence for this Codex session.

## Tools and Skills Used

- **Skills:** `superpowers:systematic-debugging`, test-driven-development and verification guidance, plus `vibin:save-to-md` for closeout.
- **Shell and GitHub CLI:** Git worktrees, Git/PR/CI inspection, builds, merge/deployment evidence, Technitium API calls, SSH, Docker/Incus, `dig`, and health probes.
- **Browser automation:** Playwright/Chromium for Loadouts and Axon navigation, console/network failure detection, and stale-client behavior.
- **Review agents:** Three scoped reviewers covered code/config/tests; review findings were addressed and re-review found no remaining actionable correctness issue.
- **Issues encountered:** Labby catalog HTTP 401, denied AXFR, backup permissions, stale primary `admin` bootstrap login, and exact-name wildcard shadowing; all are recorded below.

## Commands Executed

| command | result |
|---|---|
| `gh pr view 484 ...` / `gh pr view 481 ...` | Confirmed file manifests, merge commits, and merge times |
| `pnpm build` plus static artifact checker | Export retained exact `main` build ID in Loadouts Flight data |
| `cargo build --profile release-fast` | Produced deployed Labby binary |
| Technitium `/api/zones/records/get` and `/add` | Audited and changed both resolvers without editing binary zones directly |
| `dig @100.75.111.118`, `@100.88.16.79`, `@100.100.100.100` | All internal matrix entries ECH-free with matching private hints |
| Playwright navigation matrix | Labby through both hostnames and Axon returned HTTP 200 without request failures |

## Errors Encountered

- PR review found `deploymentId` incompatible with static serving and an unguarded synchronous Git lookup; both were replaced and covered by tests.
- CI exposed a fixed-time metrics fixture and an incomplete gateway subset fixture; both were corrected before merge.
- Labby catalog discovery returned HTTP 401 and correctly refused local fallback; direct host tools were used after reporting the boundary.
- AXFR was denied, so the authenticated official Technitium records API was used.
- A non-root zone backup copy was denied before mutation; bounded `sudo cp` created the backups.
- The primary `admin` bootstrap credential was stale; the configured `jmagar` identity authenticated through the container-managed value.
- Post-change DNS verification exposed exact-node shadowing; explicit paired records corrected it on both resolvers.

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| Static navigation | Same-version navigation could become a full-page load because deployment identity could not be compared | Same build remains client-side; stale build safely reloads using static build IDs |
| Loadouts production | Chrome displayed a load failure | Both production hostnames return HTTP 200 and render Labby |
| Internal ECH | Private destination could be paired with Cloudflare public ECH metadata | Internal apex, wildcard, and exact exceptions return ECH-free private HTTPS hints |
| Resolver redundancy | Only one corrected resolver could leave intermittent failure | Squirts, Dookie, and Tailscale synthetic DNS agree |
| Future hostnames | Required hostname-by-hostname repair | Ordinary new names inherit zone-wide wildcard records; exact exceptions are tracked for automation |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| Unit build-ID tests | resolver behavior covered | 4/4 passed during PR re-review | pass |
| PR #481 required checks | all required checks green | merge completed at `daf9caa` | pass |
| `/health`, `/ready`, `/loadouts/` | HTTP 200 | HTTP 200 after exact-main deployment | pass |
| Direct and synthetic DNS matrix | private hints, no `ech=` | all checked internal entries matched | pass |
| Public `1.1.1.1` control | Cloudflare public records unchanged | public ECH remained where configured | pass |
| Fresh Chromium | pages load without ECH/network failure | three URLs HTTP 200, zero request failures | pass |

## Risks and Rollback

- DNS changes affect both internal resolvers. Restore the four timestamped pre-change zone copies through Technitium if rollback is necessary.
- Public Cloudflare DNS was not mutated.
- The application deployment can be rolled back by reinstalling the previous binary/assets, but exact `main` passed health and browser verification.
- A Technitium credential was shared in chat and should be rotated.

## Decisions Not Taken

- Did not add `x-nextjs-deployment-id` synthesis to every static response path; static build IDs are the native mechanism for this architecture.
- Did not disable public Cloudflare ECH; internal split-DNS metadata was corrected at the authoritative internal boundary.
- Did not manually enumerate every future hostname; zone-wide records now cover ordinary names.
- Did not remove worktrees or branches with active, dirty, or unclear ownership during documentation closeout.

## References

- [PR #484](https://github.com/dinglebear-ai/labby/pull/484)
- [PR #481](https://github.com/dinglebear-ai/labby/pull/481)
- [Technitium DNS API documentation](https://github.com/TechnitiumSoftware/DnsServer/blob/master/APIDOCS.md)
- Homelab maintenance record: `/home/jmagar/workspace/homelab/docs/maintenance/technitium/2026-08-23-split-dns-ech-and-labby-recovery.md`

## Open Questions

- Whether exact split-DNS provisioning should be a Labby-owned operator action, a Technitium-specific homelab script, or declared-state automation remains to be designed in `lab-pmcvh`.
- The Labby remote catalog HTTP 401 is separate from the completed Loadouts/ECH repair and remains unaddressed in this session.

## Next Steps

- Rotate the Technitium credential disclosed in chat.
- Implement and verify bead `lab-pmcvh` before relying on manual exact-name overrides.
- Diagnose the separate Labby catalog authentication failure if remote catalog operations are needed.
