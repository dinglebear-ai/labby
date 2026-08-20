---
title: W19 Artifact Gateway Progress
created: 2026-08-19
updated: 2026-08-20
---

# W19 Artifact Gateway Progress

Last updated: 2026-08-20
Status: Phase 2 lifecycle/provider slice complete and locally reverified on current main; PR #464 CI revalidation pending

## Lane identity

- Worktree: `/home/jmagar/workspace/labby-w19-artifact-gateway`
- Branch: `codex/w19-artifact-lifecycle-20260819`
- Initial implementation baseline: `origin/main` `0e21d0474`
- Phase 2 baseline: `origin/main` `52061b650` (Phase 1 #462, Code Mode cleanup recovery #463, and usage analytics #459; #459 has no Phase 2 file overlap)
- Product: Labby AGPLv3 open personal Artifact + MCP/runtime Gateway
- Hosted registry/publication authority: Depot/Bazaar
- Crawl/enrichment authority: Axon
- Phoenix: reference/integration consumer, not Artifact authority

## Preflight evidence

Completed before W19 edits:

- verified host `dookie`, user `jmagar`, Linux x86_64;
- fetched and pruned Labby origin;
- inventoried all current worktrees, local branches, and open PRs;
- confirmed the reserved W19 worktree was clean and had no W19 PR;
- fast-forwarded W19 from its old baseline to settled `origin/main` `0e21d0474`;
- entered only the W19 isolated worktree;
- did not touch Phoenix's dirty `codex/unraid8-foundations` worktree.

## Contract decisions

Frozen and implemented:

- exact schema ID `dinglebear.artifact-interchange/v1`;
- Depot commit `25de725` is the canonical G0 v1 contract/fixture source;
- canonical JSON sorts object keys and preserves arrays;
- component inventory is canonical `{path, id}` order;
- revision ID defaults to the component-inventory SHA-256 content digest in v1, while the frozen contract permits an explicit opaque/reference revision ID;
- provenance is evidence, not trust;
- license/redistribution is separate state and defaults unknown;
- byte publication requires explicit redistributable/forkable rights;
- fork following records intent and never authorizes silent updates;
- Skills use Artifact `kind = skill` via an adapter over existing Skills validation.

Copied fixture:

- `crates/labby-runtime/tests/fixtures/artifact-interchange-v1.json` copied exactly from Depot `25de725`;
- frozen fixture revision digest: `sha256:feda49490988a21b01ea9d6548f2c893a7cea6c4e9834322985c28d82280c13f`.

## Historical mechanics reviewed

Retired product surfaces remain retired. W19 mined mechanics from history including:

- `c39bbf451`: canonical store and advisory locking;
- `76adeb9f9`: import/workspace behavior;
- `f33fbcfda`: SHA-256 revisions;
- `1564665d1`: export safety and executable mode handling;
- `f5134580a`: provider abstraction;
- historical deployment orchestration: target/plan/stage/verification/rollback and remote-target concepts.

Phoenix evidence reviewed through its current repository included bounded path-safe Artifact writes, SHA-256 content identity, versioned local state, and its docs/eight multi-file Artifact architecture.

## Implemented files

Shared runtime:

- `crates/labby-runtime/src/artifacts.rs`;
- `crates/labby-runtime/src/artifacts/canonical_json.rs`;
- `crates/labby-runtime/src/artifacts/model.rs`;
- `crates/labby-runtime/src/artifacts/validation.rs`;
- `crates/labby-runtime/src/artifacts/skill.rs`;
- `crates/labby-runtime/src/artifacts/local_io.rs`;
- `crates/labby-runtime/src/artifacts/store.rs`;
- `crates/labby-runtime/src/artifacts/store_ops.rs`;
- `crates/labby-runtime/src/lib.rs` module projection.

Tests/fixtures:

- `crates/labby-runtime/tests/artifact_interchange_conformance.rs`;
- `crates/labby-runtime/tests/fixtures/artifact-interchange-v1.json`.

Documentation:

- `docs/artifacts/spec.md`;
- `docs/artifacts/contract.md`;
- `docs/artifacts/implementation-plan.md`;
- `docs/artifacts/progress.md`.

## First-slice capabilities

Implemented:

- multi-file package snapshot/import;
- SHA-256 file digests;
- deterministic Artifact/component IDs;
- canonical ordered component inventory;
- immutable revision storage;
- mutable head record and separate workspace;
- provenance/license/publication/lineage state;
- per-Artifact OS file lock;
- symlink/path containment protection;
- file/package/component/metadata bounds;
- stored-byte size and digest verification before export/fork;
- secret-like text export blocked by default;
- explicit force/include-secrets opt-ins;
- fork pins exact source revision;
- upstream observation without byte mutation;
- existing SEP-2640 Skill manifest/digest verification reused for Artifact Skill projection.

No migration was added because this is a new local Artifact store and no published current state requires migration.

## Test evidence

Phase 1 Artifact evidence from the settled implementation baseline, preserved after rebase through current `origin/main` `ea07f3609`:

- `cargo fmt --all -- --check`: passed.
- `git diff --check origin/main...HEAD`: passed.
- `cargo test -p labby-runtime --all-features -- --nocapture`: 160 unit tests + 1 agent-error schema + 2 ArtifactInterchange conformance + 11 SEP-2640 URI conformance + 8 Skills contract conformance = 182 passed, 0 failed.
- `cargo clippy -p labby-runtime --all-features --all-targets -- -D warnings`: passed.
- `RUSTFLAGS='-D warnings' cargo check -p labby-runtime --all-features --all-targets`: passed.
- pinned fleet repository contract `218eba19f15cc13554d26fa131309cfa8141fd67`, profile `rust`: passed locally after adding required durable-doc frontmatter.
- focused Artifact unit coverage within the full run: 20 passed, 0 failed after adversarial hardening.
- exact Depot fixture conformance: 2 passed, 0 failed; byte-canonical round-trip and frozen revision digest match.

Compilation findings already fixed:

- Rust 1.97 SHA-256 output formatting compatibility;
- `Read::by_ref` ambiguity;
- Rust 1.97 `std::fs::TryLockError` handling;
- existing Skills digest API uses `ResourceDigest::of_bytes(...).to_wire()`;
- warnings lint for discarded cleanup result;
- frozen schema constant re-export for conformance tests.

Phase 1 implementation is complete and merged as PR #462 at `87239104c`. The first remote repository-contract run exposed missing required frontmatter on the four new durable Artifact docs; that failure was reproduced with the exact pinned validator and fixed. After the branch rebased across the shared Skills compatibility facade (#456), remote CI exposed runner-memory SIGKILLs in Clippy, feature slices, Linux Test, and focused MCP regressions. Current `main` reproduces the same Test/MCP/gateway SIGKILL class. W19 now phase-separates ordinary `labby` target warm-up from all-target/test-harness fan-out without setting `CARGO_BUILD_JOBS` or command-local `-j 1`. The pinned fleet policy/contract, forbidden-architecture scan, Actionlint, workflow-contract assertions, format, and diff hygiene pass locally. The exact staged Clippy workflow passed status 0 with phases completing in 32.56s, 16.37s, and 1m30s. The gateway feature slice passed warm compile (1m05s), all-target compile (1m00s), and 1,272/1,272 tests with 3 skipped; the fs slice completed its warm/all-target phases and the focused proxy preflight passed 6/6 tests. No SIGKILL occurred in the staged verification.

## Adversarial review findings addressed

- Corrected frozen revision semantics: Depot sorts components for digesting and permits an explicit revision reference ID; Labby no longer incorrectly requires `id == contentDigest` or pre-sorted input.
- Replaced Labby's broader generic redaction-key matcher with the exact frozen Artifact metadata secret-key semantics so portable fields such as `code`, `cwd`, and `public_key` are not rejected while `credential`, token/password/API-key shapes still fail closed.
- Bound Skill Artifact identity to the canonical Skill URI to prevent nested Skills with the same final name from colliding.
- Removed silent provenance-version coercion from the Skill adapter. Unsupported versions now fail contract validation.
- Hardened hashed store/revision/files/lock paths against symlink substitution and made the personal store/locks private on Unix.
- Validated the nearest existing store ancestor before directory creation so a rejected sensitive/symlinked root is not mutated first.
- Refused export destinations that overlap the canonical Artifact store, even with force enabled.
- Added directory-depth and aggregate directory-entry traversal budgets before collection can grow unbounded.
- Added bounded reads for mutable head JSON and immutable revision manifests, plus a revision-history count ceiling.
- Kept the existing textual secret detector as the stronger local export guard while preserving the narrower frozen cross-product metadata rule.

Residual hardening note: Labby's shared path-safety helpers are path/canonicalization based and explicitly document a concurrent filesystem TOCTOU window. The first slice rejects static traversal/symlinks and is appropriate for the personal trusted local-store threat model. FD-relative/openat-style no-follow traversal should precede treating hostile concurrent local filesystem mutation as supported.

## Commits and PR

Current rebased W19 checkpoints:

- `21e24832b` — `feat(artifacts): add local Artifact core and v1 contract`;
- `7394d6489` — `docs(artifacts): record W19 slice verification`;
- `7dce403cc` — `docs(artifacts): record W19 pull request`;
- `2e4068479` — `docs(artifacts): satisfy fleet frontmatter contract`.

W19 PR: #462, `feat(artifacts): add open personal Artifact core and v1 interchange`.


## Phase 2 lifecycle/provider checkpoint

Phase 1 merged as PR #462 at `87239104c84874694748ed7135919e11a8d76d4b`. Phase 2 continues in the same W19 worktree on `codex/w19-artifact-lifecycle-20260819`, rooted directly at that merged commit.

Implemented in the Phase 2 slice:

- deterministic path-ordered revision diffs;
- editable-workspace snapshots into immutable revisions;
- exact historical revision reuse for repeated/reverted content;
- expected-base compare-and-swap protection for mutable head transitions;
- transport-neutral `ArtifactProvider` / `ArtifactAcquisition` contracts and a verified local-store provider;
- provider file/package byte budgets plus exact path/size/SHA-256 verification;
- explicit read-only update plans that retain local base, source Artifact/revision, source provenance, and the component diff without applying bytes;
- safe cross-platform atomic JSON replacement using `atomic-write-file`, replacing the previous Windows delete-then-rename fallback while preserving Labby's symlink checks and private Unix file mode.

Adversarial review caught and fixed a content-reversion parent-cycle bug before checkpointing. A snapshot now derives content identity before adding parent linkage, so returning to old content selects the exact prior immutable revision. Provider payloads were also hardened to the same local file/package budgets as imports, and mutable-head replacement no longer has a Windows visibility gap.

Final verification: focused hardened Artifact tests passed 27/27; the full `labby-runtime` all-features suite passed; Clippy passed with `-D warnings`; `RUSTFLAGS=-D warnings` all-target check passed; `cargo deny check` passed advisories, bans, licenses, and sources; the stale-head compare-and-swap regression passed; `labby docs check` reported 17 generated docs fresh; the documentation link checker verified 347 local links; rustfmt and `git diff --check` passed. The detached post-doc verifier exited `0` with `FINAL_OK`. After #463 advanced `main` to `bb30616f4`, Phase 2 rebased cleanly with zero file overlap. The post-rebase verifier then passed full `labby-runtime` tests (169 unit + 1 schema + 2 ArtifactInterchange + 11 SEP-2640 + 8 Skills contract), Clippy `-D warnings`, `RUSTFLAGS=-D warnings` all-target check, `cargo deny check`, 17 fresh generated docs, 347 local links, rustfmt, and `git diff --check`, ending with `POSTREBASE_OK`. After #459 advanced `main` again to `52061b650`, Phase 2 performed a second clean zero-overlap rebase. The second-rebase code verifier passed the same full `labby-runtime` suite, Clippy `-D warnings`, `RUSTFLAGS=-D warnings` all-target check, rustfmt, `git diff --check`, and `cargo deny check`, ending with `SECOND_REBASE_CODE_OK`.

Phase 2 PR: #464, `feat(artifacts): add lifecycle planning and provider seam`; rebased onto current `main` `52061b650` after #459 merged.
