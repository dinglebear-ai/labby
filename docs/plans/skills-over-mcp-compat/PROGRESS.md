# Progress: Skills over MCP compatibility

Status: in progress
Branch: feat/skills-over-mcp
Worktree: /home/jmagar/workspace/labby-skills-over-mcp
Started: 2026-08-18
Last updated: 2026-08-19 16:04 America/New_York

This file is the living tracker. Update it whenever implementation state, decisions, verification, or rebase risk changes.

## Current state

### Completed discovery

- [x] Confirmed DOOKIE target: hostname dookie, user jmagar, Linux/Ubuntu host.
- [x] Confirmed source checkout /home/jmagar/workspace/labby.
- [x] Fetched current origin/main.
- [x] Created fresh worktree /home/jmagar/workspace/labby-skills-over-mcp.
- [x] Created branch feat/skills-over-mcp from origin/main at 8d0a39bd4.
- [x] Read root and nested MCP/dispatch architecture guidance.
- [x] Audited existing native Skills implementation instead of assuming greenfield.
- [x] Confirmed current main already has SEP-2640 runtime vocabulary, native server methods, first-party/local skills, upstream aggregation/cache/verification, resource reads, and conformance tests.
- [x] Confirmed shared CLI/API Skills compatibility service is absent on main.
- [x] Confirmed no skills.search/read compatibility action exists on main.
- [x] Identified adjacent unmerged feature/skills-ui-config branch and commit 73f160dad adding gateway Skills/loadouts configuration and UI. Do not duplicate or overwrite it.
- [x] Checked upstream SEP status through Labby's GitHub upstream: PR 2640 remains open; experimental draft file currently resolves at repository snapshot f1f66fa7f8c75d6094dff1fd4a5e83f058ec8692, blob 6b535330430f55170bab488dde661f8909fb947b.

### Documentation package

- [x] README.md created.
- [x] SPEC.md created.
- [x] CONTRACT.md created.
- [x] IMPLEMENTATION_PLAN.md created.
- [x] PROGRESS.md created and designated as living tracker.
- [x] Added plan package to docs/README.md index.

### Implementation

- [x] P0 baseline and compatibility verification recorded: runtime conformance, Gateway-enabled compilation, Skills-only compilation, focused Skills tests, annotation/F9 drift guards, and docs generation/check are green.
- [x] P1 extract canonical Skills semantics out of MCP-only ownership: first-party/local registry, origin minting, metadata search, and caller-scoped list/get/read facade now live under `crate::skills` and are consumed by native MCP plus compatibility adapters.
- [x] P1 create shared Skills dispatch catalog, parameter validation, stable response types, and first-party-safe context builder.
- [x] P1 implement skills.list across shared dispatch; native SEP listing now delegates to the same caller-scoped facade.
- [x] P1 implement skills.search with deterministic metadata-only ranking and fixed-size service exposure.
- [x] P1 implement skills.get against the canonical caller-scoped registry.
- [x] P1 implement skills.read against the canonical manifest-bound verified reader.
- [x] P1 register exactly one Skills compatibility service. Direct MCP calls inject caller route/OAuth context; Code Mode carries verified caller authorization plus route-visible upstream scope through private in-process metadata and fails closed if either is absent.
- [x] P1 add CLI and HTTP API thin adapters. HTTP mounts only with API auth; `help`/`schema` are authenticated catalog introspection while list/search/get/read require `lab:read`, `lab`, or `lab:admin`, matching the MCP compatibility contract. CLI uses root local-gateway scope when Gateway is compiled.
- [x] P1 first-party parity tests native vs compatibility: get/read identity, native manifest verification, deterministic listing/search, and read authorization are green. Live proxied parity remains part of the final E2E smoke matrix.
- [x] P1 route/auth isolation unit coverage: context-free upstream lookup is denied, protected-scope facade allowlisting is green, unauthenticated HTTP is denied, read-scoped HTTP is allowed, non-read HTTP callers can inspect only help/schema, and private Code Mode scope propagation round-trips. Live route isolation remains part of the final E2E smoke matrix.
- [~] P1 observability: compatibility calls use the shared dispatch logs, auth denials log surface/service/action/request id, collision exclusions log upstream/skill, and live log evidence remains to capture during E2E.
- [x] P1 repository verification: focused Skills/API suite is 52/52 green; permanent-tool annotation/F9 module is 11/11 green; runtime Skills contract conformance is 8/8 green; architecture service inventory is green; final post-fix `cargo nextest run --workspace --all-features --no-fail-fast` is 3094/3094 passed with 8 skipped; generated docs are 17/17 fresh; all-features workspace check and Skills-only feature slice compile are green.

## Decisions

### D-1 One canonical registry, multiple projections

Accepted. Native SEP, fixed tool, Code Mode, resources, and later filesystem projection all use one source of truth.

### D-2 No per-skill tools

Accepted. Labby exposes one Skills service/tool independent of skill cardinality.

### D-3 Native SEP remains preferred

Accepted. Compatibility projection is fallback/adaptation, not a replacement for io.modelcontextprotocol/skills.

### D-4 Fixed tool actions

Accepted for P1: skills.list, skills.search, skills.get, skills.read.

### D-5 Search is metadata-only

Accepted. P1 does not fetch SKILL.md bodies for ranking.

### D-6 Filesystem projection is deferred

Accepted. It requires explicit mutation, conflict policy, provenance manifests, and client-path adapters. It will be built only after the shared registry/actions are stable.

### D-7 Adjacent skills-ui-config work stays separate

Accepted. This branch will expose integration seams but will not cherry-pick 73f160dad wholesale without review because that commit spans gateway config, protected routes, loadouts, CLI, and web UI.

## Findings that changed the plan

1. Skills over MCP is not new to Labby. The native extension is already substantial and security-sensitive. The compatibility project is primarily a refactor/projection exercise.
2. First-party skill ownership currently lives in crates/labby/src/mcp/skills.rs, which makes a naive CLI/API implementation likely to duplicate logic. P1 must extract shared semantics before adding adapters.
3. The verified upstream read path is currently text-shaped. Compatibility skills.read therefore starts text-only and must return a structured unsupported-content error for other shapes until the canonical reader is generalized.
4. Code Mode does not require a second Skills protocol. A registered internal Skills service gives tool-only models a compatibility path even when direct tools are hidden.
5. The existing `docs/contracts/skills-extension.md` table contained an invalid mirror SHA. The intended pin is `9f55cd349932ba00fc18402873c9eb2d2c2e78cb`; current mirror snapshot `f1f66fa7f8c75d6094dff1fd4a5e83f058ec8692` serves the identical SEP draft blob `6b535330430f55170bab488dde661f8909fb947b`, so there is no normative wire drift. The contract provenance has been corrected.
6. Code Mode's in-process built-in peers intentionally have `gateway_manager: None`. This branch now propagates a private, mint-never-forward pair of caller authorization plus allowed-upstream scope from the outer Code Mode host; the mini-peer reconstructs a caller-scoped registry only for the trusted in-process transport and fails closed to first-party-only when either value is absent. Root Code Mode can therefore expose proxied Skills without erasing OAuth/route boundaries.
7. The documented `--no-default-features --features skills` product slice was already broken on the original `origin/main`: the no-gateway `proxied_skill_entries` stub returned `Vec<SkillEntry>` while its caller consumed `ProxiedSkills`, and the no-gateway `read_proxied_skill_file_impl` stub accepted four arguments while the shared caller passed five. This branch fixes those baseline defects without changing gateway/native behavior.
8. Adversarial review found an unnecessary transport divergence: API `help`/`schema` required Skills read scope while MCP allowed catalog introspection. API now matches MCP: authenticated callers may inspect help/schema, but list/search/get/read remain read-scoped.
9. Adversarial review found a URI-ownership edge case for unlisted `skills/get`: an excluded or unlisted manifest could reuse a supporting-file URI already owned by another skill. Collision handling now poisons every URI owned by an excluded skill and rejects unlisted candidates that overlap published or poisoned ownership.
10. Adding the fixed `skills` service correctly tripped permanent-tool annotation and next-hop authorization drift guards. The service is explicitly reviewed as read-only, non-destructive, idempotent, open-world; all six actions are pinned so any future mutation forces re-review.
11. Post-#448 adversarial review found a loadout bypass: a protected route could allow the `skills` service name while `expose_skills=false`, letting the fixed compatibility tool remain listable/callable even though native SEP and resource reads were disabled. MCP service visibility now centrally treats the Skills capability gate as part of `skills` service visibility; a regression proves both `tools/list` hiding and forged `tools/call` denial.

## Verification log

Verification to date:

- `cargo test -p labby-runtime --test skills_contract_conformance`: 8/8 passed against corrected mirror pin `9f55cd349932ba00fc18402873c9eb2d2c2e78cb`.
- `cargo check --workspace --all-features`: green; Gateway-enabled `labby --features skills --lib`: green; `labby --no-default-features --features skills --lib`: green.
- `labby` all-features Skills-filtered tests: 51/51 passed, including API auth parity, metadata-only search, deterministic list/search, native SEP serving, collision hardening, and first-party read identity.
- `mcp::permanent_tools::tests`: 11/11 passed after explicitly reviewing/pinning the `skills` service annotations and F9 next-hop reachability.
- `architecture_orchestrator services_list_is_current`: passed after classifying `skills` as a shared-dispatch service.
- Final post-rebase/post-hardening `cargo nextest run --workspace --all-features --no-fail-fast`: 3094/3094 passed, 8 skipped; five pre-existing gateway-manager tests were slow but passed.
- Generated docs were regenerated; `labby docs check` reported 17 artifacts fresh.
- Final debug binary built at `target/debug/labby`. Legacy stdio MCP compatibility smoke passed for `2024-11-05`, `2025-03-26`, `2025-06-18`, and `2025-11-25`: every version echoed correctly, advertised exactly one `skills` compatibility tool in a two-tool scoped catalog, and completed `skills.list`, `skills.get`, and `skills.read` for a bundled skill.
- `cargo fmt --all` and `git diff --check` are clean after the latest fixes.
- Build orchestration note: Kache/RUSTC_WRAPPER contention caused multi-minute stalls, so verification commands explicitly clear `RUSTC_WRAPPER` / `CARGO_BUILD_RUSTC_WRAPPER` and keep Cargo jobs sequential.

## Rebase watch

High-churn adjacent areas:

- crates/labby/src/mcp/skills.rs
- crates/labby/src/mcp/server.rs
- crates/labby/src/mcp/handlers_resources.rs
- crates/labby-gateway/src/upstream/pool/skills.rs
- crates/labby-gateway/src/gateway configuration and projection files
- apps/gateway-admin Skills/loadouts pages on feature/skills-ui-config

Before each rebase, inspect main for changes to shared Skills types or route/loadout exposure. The remote `feature/skills-ui-config` branch was deleted by the 2026-08-19 fetch; preserve any equivalent work that has landed on main rather than resurrecting the deleted branch. Never resolve conflicts by dropping security/route checks.

Rebase state as of 2026-08-19 16:04 America/New_York:

- [x] fresh `origin/main`: `0e21d0474`; `git rev-list --left-right --count HEAD...origin/main` reports `3 0`, so the feature contains current main and does not need another rebase;
- [x] local `main` ref was safely fast-forwarded again to `0e21d0474`;
- [x] no worktree checks out `main`; the primary `/home/jmagar/workspace/labby` checkout remains on `fix/persist-plugin-server-url` and was not disturbed;
- [x] incoming #448 (`bdf914c15`, first-class Skills over MCP and Loadouts) is beneath this compatibility work. The sole textual conflict was `mcp/handlers_resources.rs`; resolution preserved #448 loadout `exposes_skills()` gating plus the shared facade/read-scope path;
- [x] pushed branch head: `c16a40004`; draft PR #456 is open against `main`.

## Next implementation step

Run clippy/lint and safe live proxied/native/compatibility E2E smokes, update PR #456 evidence, perform a fresh adversarial review of the full PR diff, address every finding, rerun affected gates and CI, mark the PR ready, merge to main, then verify settled `main` and local-main synchronization.
