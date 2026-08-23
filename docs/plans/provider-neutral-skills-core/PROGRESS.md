# Provider-neutral Skills core progress

Last updated: 2026-08-22
Branch: `codex/provider-neutral-skills-core`
Worktree: `/home/jmagar/workspace/labby/.worktrees/codex/provider-neutral-skills-core`
Tracking: `lab-27juw`

## Verified baseline

- Fresh `origin/main`: `daf9caa488d5e3de0b236a7984ef550bfbfe6031`.
- Main includes #481 operator diagnostics and catalog browsing.
- The referenced ChatGPT session created no repository worktree or documents.
- Existing `docs/plans/skills-over-mcp-compat/` addresses client projection,
  not provider neutrality; it remains an adjacent plan.
- Current operator state is `OperatorSkill { skill, exposed: bool }`.
- Current enforcement compiles `expose_skills` through the shared fail-closed
  matcher and applies it both to cached listings and unlisted direct fetches.

## Ledger

- [x] Reconstruct prior design and distinguish hidden from rejected skills.
- [x] Inspect current Skills wire contract and compatibility plan.
- [x] Create fresh worktree and tracking bead.
- [x] Freeze spec, contract, implementation plan, and progress tracker.
- [x] Implement exposure decision v1.
- [x] Run focused decision/projection tests.
- [x] Run gateway test and Clippy gates.
- [x] Introduce a dedicated `SkillExposurePolicy` without changing persisted
  `expose_skills` syntax or defaults.
- [x] Preserve one shared matcher/compiler and fail-closed behavior.
- [x] Verify listing, direct get/read, operator decision, and downstream fixture
  compatibility after the policy migration.
- [x] Add provider-scoped `SkillId`, `SkillProviderId`, provider kinds, and a
  compact provider-neutral `SkillDescriptor` in `labby-runtime`.
- [x] Adapt validated SEP entries without fetching instruction/file bodies.
- [x] Project the descriptor identity through the operator Skills boundary.
- [x] Define the bounded provider discovery/get/read trait contract.
- [x] Add a caller-scoped SEP provider adapter that delegates to the existing
  cache, exposure, validation, direct-get, digest, and frontmatter paths.
- [x] Add bundled and operator-local providers without changing compatibility
  URI precedence.
- [x] Extend provider discovery/get results with validated author metadata,
  resource identities/digests, TTL, and exclusion bookkeeping required for an
  exact compatibility projection; resource bodies remain lazy.
- [x] Route upstream list aggregation through `SepSkillProvider::discover`
  while preserving existing minting, collision, TTL, and incomplete metadata.
- [x] Add fail-closed compatibility/availability vocabulary without embedding
  execution authorization.
- [x] Route exact upstream get/read through `SepSkillProvider` for listed Skills.
- [ ] Make direct-get unlisted Skill snapshots readable without weakening
  manifest ownership, then migrate first-party compatibility projection fully
  through providers.

## Decisions

- Preserve the SEP-2640 implementation as a provider adapter.
- Preserve `exposed` in operator JSON during migration.
- Do not interpret vendor `allowed-tools` as authorization.
- Do not require remote Skills to become Artifacts merely to be consumed.
- Do not change exposure defaults in the status-model slice.

## Verification

- `cargo fmt --all -- --check`: passed.
- Exposure decision unit tests: 3 passed.
- Operator exposed/hidden decision integration test: passed.
- `cargo test -p labby-gateway --features skills`: 891 passed, 5 ignored,
  0 failed; doc tests passed.
- `cargo clippy -p labby-gateway --all-features --all-targets -- -D warnings`:
  passed.

### Slice 2 verification

- `cargo check -p labby-gateway --all-features --all-targets`: passed.
- `cargo check -p labby --all-features --all-targets`: passed; the fresh
  worktree emitted the expected advisory that the optional prebuilt web bundle
  is absent.
- Dedicated policy/default/fail-closed tests: passed.
- Existing `expose_skills` update/filter and hidden-file read guards: passed.
- Full gateway suite: 894 passed, 5 ignored, 0 failed; doc tests passed.
- All-features/all-targets gateway Clippy with warnings denied: passed.

### Slice 3 verification

- Provider collision, metadata-preservation, progressive-disclosure, and JSON
  identity tests: 3 passed.
- `labby-runtime` all-features/all-targets Clippy with warnings denied: passed.
- Operator integration test confirms MCP upstream identity and native source URI
  survive descriptor adaptation.
- Full `labby-runtime` suite: 177 unit tests plus 22 contract/integration tests
  passed; doc tests passed.
- Full gateway suite after descriptor integration: 894 passed, 5 ignored,
  0 failed; doc tests passed.
- Gateway all-features/all-targets Clippy with warnings denied passed after the
  descriptor integration.

### Slice 4 verification in progress

- Provider contract request/result validation and object-safety tests: 4
  passed.
- SEP adapter focused test: passed.
- The adapter retains the OAuth subject in its instance and wraps operations in
  the caller deadline. Discovery returns no more than `max_items`.
- SEP pagination remains bounded by the provider's fixed incremental host cap;
  the neutral request controls returned items and deadline without pretending
  that cached traversals honor caller-selected page counts.
- Bundled and operator-local providers snapshot validated entries and exact
  bytes behind distinct provider IDs while leaving the current first-party
  compatibility registry and bundled-first collision behavior unchanged.
- Bundled progressive discovery/read and cross-provider identity tests: 2
  passed.
- Full runtime suite: 181 unit tests plus 22 contract/integration tests passed;
  doc tests passed.
- Full gateway suite: 895 passed, 5 ignored, 0 failed; doc tests passed.
- Runtime, gateway, and product-crate all-features/all-targets Clippy with
  warnings denied: passed. The product build emitted only the expected advisory
  that the optional prebuilt gateway-admin web bundle is absent.

### Subsequent hardening and cutover work

- Bound provider resource requests to 1 MiB across source types and check
  snapshot sizes before cloning bytes.
- Bind SEP reads to both the provider-scoped Skill identity and exact resource
  identity; a cross-Skill read regression now passes.
- Preserve stable digest/stale/limit classifications at the provider boundary
  instead of flattening every read failure into a generic provider error.
- Provider discovery now carries validated manifest metadata separately from
  compact descriptors, and the upstream list facade consumes that projection.
- Compatibility/availability classification tests: 3 passed.
- Provider contract tests: 4 passed; focused facade tests: 4 passed; bundled
  provider tests: 2 passed; cross-Skill owner-binding test: passed.
- Product-crate all-features/all-targets Clippy passed after upstream
  list/get/read facade cutover.
