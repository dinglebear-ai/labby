# Implementation plan: universal Skills over MCP compatibility

Status: active
Created: 2026-08-18
Last updated: 2026-08-18

## 1. Delivery strategy

Implement in small parity-preserving layers. Native SEP behavior is already working code, so every refactor phase must prove equivalence before adding a new projection.

Order:

1. lock baseline
2. extract shared canonical semantics
3. add fixed compatibility actions
4. register thin adapters
5. add interoperability harnesses
6. add optional filesystem projection
7. integrate operator UI/loadouts work

Do not jump directly to client-specific exporters while canonical logic still lives under MCP-only modules.

## 2. Phase 0: baseline and drift lock

### P0.1 Record current base

- branch feat/skills-over-mcp
- base origin/main 8d0a39bd4
- record git status and relevant feature flags

### P0.2 Revalidate SEP pin

- compare docs/contracts/skills-extension.md pinned revision with current experimental-ext-skills main
- determine whether current draft bytes changed in any normative section since Labby's pin
- if unchanged, record evidence only
- if changed, create a native-conformance subtask before changing runtime behavior

### P0.3 Run existing targeted tests

At minimum:

- labby-runtime Skills contract/conformance tests
- labby-gateway upstream pool Skills tests with skills feature
- labby first-party/native Skills tests with skills feature
- relevant resource-read tests

Baseline failures must be classified before refactoring. Do not hide pre-existing failures by rewriting tests around the new design.

### P0.4 Update PROGRESS.md

Record exact commands and results.

## 3. Phase 1: extract canonical first-party/local registry

### Goal

Make bundled/local skill list/get/read semantics reusable without changing the native extension.

### P1.1 Create transport-neutral product module

Preferred location inside the labby product crate: crates/labby/src/skills.rs plus focused submodules, or another location consistent with current architecture review.

Move or wrap:

- embedded first-party file table
- FirstPartySkill representation
- first-party URI construction
- first-party/local registry initialization
- list_first_party_skills
- first_party_skill_entry
- read_first_party_skill_file

Do not move rmcp request handling into the shared module.

### P1.2 Preserve native adapter

crates/labby/src/mcp/skills.rs becomes a native protocol adapter plus aggregate/origin logic that calls the shared registry.

Parity tests compare serialized first-party listing and file bytes before and after extraction.

### P1.3 Build aggregate facade boundary

Create a reusable request facade that can combine:

- first-party registry
- gateway manager and pool
- route scope
- caller subject

The facade must expose list/get/read operations without depending on rmcp RequestContext.

If extracting route/auth context in one step creates excessive churn, land first-party extraction first and keep proxied compatibility disabled behind tests until the facade exists.

## 4. Phase 2: shared Skills dispatch service

Follow crates/labby/src/dispatch/CLAUDE.md.

Required layout:

- dispatch/skills.rs
- dispatch/skills/catalog.rs
- dispatch/skills/client.rs or a documented sanctioned local-runtime equivalent if no external client exists
- dispatch/skills/params.rs
- dispatch/skills/dispatch.rs

Because this service fronts a local/gateway runtime rather than an HTTP API, request a deliberate architecture exception only if client.rs is genuinely meaningless. Prefer an explicit facade/client object over casually skipping structure.

### P2.1 Action catalog

Define skills.list, skills.search, skills.get, and skills.read. All are non-destructive and read-scoped.

### P2.2 Parameter validation

Implement bounded parameters:

- list limit 1 through 500
- search query non-empty
- search limit 1 through 100
- origin optional but validated against visible labels
- get/read URI required and syntactically valid

### P2.3 Shared results

Define serializable summaries/results in the narrowest reusable crate that needs them. Do not leak rmcp model types into dispatch.

### P2.4 Search implementation

Implement deterministic metadata ranking from CONTRACT.md. No embeddings in P2.

### P2.5 Shared verification

skills.read must call the exact canonical verified read path used by native resources/read. If that requires extracting a shared VerifiedSkillFile abstraction, do so rather than duplicating verification.

## 5. Phase 3: MCP and Code Mode projection

### P3.1 Register one Skills service

Register the shared dispatch service in registry.rs behind the existing skills feature.

Expected direct MCP tool count increase: exactly one.
Expected increase with N additional skills: zero.

### P3.1a Propagate caller scope before enabling proxied compatibility

The context-free registry `DispatchFn` is not sufficient for proxied Skills. Its safe default must expose no upstream origins.

For direct MCP calls, the thin adapter passes the live gateway manager, route-visible upstream set, and OAuth subject into a transport-neutral Skills request context before dispatch.

Code Mode uses mini in-process MCP peers whose production construction deliberately sets `gateway_manager: None` and an empty upstream allowlist. Do not reach around that boundary with process-global gateway state. Add an explicit trusted scope-propagation seam from the outer Code Mode execution into the Skills in-process call before claiming proxied Code Mode parity. Until then, Code Mode compatibility is first-party/local only.

Tests must prove the context-free path cannot discover an upstream-only skill.

### P3.2 Code Mode

Verify Code Mode search finds the Skills service and describe shows the four actions. Verify Code Mode call of skills.search and skills.read succeeds against a first-party skill.

### P3.3 Native parity

Native extension remains advertised and functional alongside the fallback tool.

Tests:

- native skills/list identity set equals compatibility skills.list identity set for the same context
- native skills/get entry equals compatibility skills.get entry
- native resources/read bytes equal compatibility skills.read text

## 6. Phase 4: CLI and API thin adapters

### P4.1 CLI

Add a top-level skills command or repository-standard equivalent with list, search, get, and read. CLI calls shared dispatch only. Human rendering is separate from JSON output.

### P4.2 API

Add the repository-standard service route and call shared dispatch/facade only. Do not rebuild pagination, search, route checks, or errors in the route handler.

### P4.3 Generated docs

Regenerate authoritative CLI/MCP catalog snapshots using repo tooling. Do not hand-edit generated files.

## 7. Phase 5: security and route isolation

Tests must prove:

- route A cannot list/search/get/read route B-only skills
- a hidden URI is indistinguishable from unknown to the hidden caller
- caller subject is used for upstream cache/token selection but never returned raw
- expose_skills filters apply to direct lookup as well as list/search
- manifest bypass attempts fail
- digest mismatch and stale manifest preserve rediscovery recovery semantics
- allowed-tools provenance metadata is not reinterpreted across origins

## 8. Phase 6: performance and scale

### P6.1 Large synthetic registry

Test at least 500 skills, 5,000 skills, and 50,000 metadata entries where practical without upstream network calls.

Measure list serialization time, search time, peak memory, and Code Mode catalog size impact.

The service tool description and action catalog must remain constant size as skill count grows.

### P6.2 Cache behavior

Verify repeated list/search operations reuse canonical upstream caches and do not create a per-query upstream storm.

## 9. Phase 7: interoperability harness

Build fixtures/probes by capability profile rather than vendor name.

### Native profile

Use a known current SEP client such as fast-agent when practical. Verify capability discovery, list/get/read, and integrity handling.

### Tool-only profile

Use a minimal generic MCP client that knows only tools/list and tools/call. Verify complete skill discovery and loading through the one Skills tool.

### Code Mode profile

Verify search/describe/call through Labby Code Mode.

### Resource-aware profile

Verify manual skill resource reads without claiming native registration.

Record current vendor-client observations separately because support changes quickly.

## 10. Phase 8: filesystem projection

Only after earlier phases are stable.

### P8.1 Generic projector

Inputs: canonical visible skill URIs, destination root, conflict policy, dry-run.

Outputs: staged verified files, provenance manifest, atomic activation result.

### P8.2 Client path adapters

Small adapters discover and validate local destinations for Claude Code, Codex, Gemini CLI, or future clients. They do not alter skill semantics.

### P8.3 Safety

- no implicit overwrite
- no uncontrolled recursive delete
- preserve unmanaged files
- atomic directory swap where platform supports it
- clear rollback artifact

## 11. Phase 9: integrate first-class gateway UI/loadouts

Review feature/skills-ui-config after shared service stabilizes.

Prefer reusing proxy_skills/expose_skills configuration, operator Skills views, and loadout capability selection.

Avoid duplicate endpoints/actions if that branch already owns operator-only views such as gateway.skills.list. Agent-facing skills.list/search and operator-facing gateway.skills.list have different audiences and may coexist if their contracts remain clear.

## 12. Testing matrix

Every significant phase runs:

- cargo fmt/check as appropriate
- targeted cargo nextest tests
- feature slice for skills without gateway where supported
- gateway plus skills slice
- workspace all-features checks before PR
- clippy with repository lint policy
- architecture tests
- docs/generated consistency checks

Before PR, run the repository's full required verification path unless an unrelated environment failure prevents it; document any blocker precisely.

## 13. Documentation updates

During implementation keep current:

- this plan
- PROGRESS.md
- SPEC.md when scope changes
- CONTRACT.md when public behavior changes
- docs/surfaces/MCP.md
- docs/surfaces/CLI.md once CLI lands
- API docs once routes land
- docs/dev/OBSERVABILITY.md for new action boundaries
- docs/dev/ERRORS.md for any new stable error kind
- docs/README.md index

Do not modify docs/sessions or docs/superpowers as part of this project.

## 14. PR and review gates

Before PR:

- rebase on current main
- inspect feature/skills-ui-config overlap
- run full verification
- inspect diff for accidental generated/reference/session changes
- secret scan relevant diff
- create PR with architecture and compatibility matrix
- perform adversarial review focused on route leakage, digest bypass, duplicate registries, unbounded fan-out, and native/fallback divergence
- address every actionable review finding

## 15. Definition of done

The project is complete when native SEP clients still work without regression, a generic tools-only MCP client can discover and load any visible text skill through one fixed service, Code Mode can do the same without direct tool exposure, CLI/API call the same shared operations, no projection duplicates canonical skill state, route/auth/exposure/integrity checks are identical across projections, scale tests demonstrate tool cardinality does not grow per skill, any shipped filesystem projection is explicit and provenance-preserving, and the documentation package reflects final behavior and verification evidence.
