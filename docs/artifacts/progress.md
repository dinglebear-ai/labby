# W19 Artifact Gateway Progress

Last updated: 2026-08-19
Status: first coherent vertical slice implemented, adversarially reviewed, and verified; ready for PR creation

## Lane identity

- Worktree: `/home/jmagar/workspace/labby-w19-artifact-gateway`
- Branch: `codex/w19-artifact-gateway-20260819`
- Initial implementation baseline: `origin/main` `0e21d0474`
- Final rebased baseline: `origin/main` `fc4d3a1c2`
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

Final green evidence on rebased `origin/main` `fc4d3a1c2`:

- `cargo fmt --all -- --check`: passed.
- `git diff --check origin/main...HEAD`: passed.
- `cargo test -p labby-runtime --all-features -- --nocapture`: 160 unit tests + 1 agent-error schema + 2 ArtifactInterchange conformance + 11 SEP-2640 URI conformance + 8 Skills contract conformance = 182 passed, 0 failed.
- `cargo clippy -p labby-runtime --all-features --all-targets -- -D warnings`: passed.
- `RUSTFLAGS='-D warnings' cargo check -p labby-runtime --all-features --all-targets`: passed.
- focused Artifact unit coverage within the full run: 20 passed, 0 failed after adversarial hardening.
- exact Depot fixture conformance: 2 passed, 0 failed; byte-canonical round-trip and frozen revision digest match.

Compilation findings already fixed:

- Rust 1.97 SHA-256 output formatting compatibility;
- `Read::by_ref` ambiguity;
- Rust 1.97 `std::fs::TryLockError` handling;
- existing Skills digest API uses `ResourceDigest::of_bytes(...).to_wire()`;
- warnings lint for discarded cleanup result;
- frozen schema constant re-export for conformance tests.

Phase 1 implementation and local verification are complete. PR #462 is open against `main`; only remote CI/merge-state observation remains.

No implementation or local verification gate remains open for Phase 1.

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

W19 core checkpoint: `036bfb94c` (`feat(artifacts): add local Artifact core and v1 contract`).
W19 verification/progress checkpoint: `4ba260910` (`docs(artifacts): record W19 slice verification`).
W19 PR: #462, `feat(artifacts): add open personal Artifact core and v1 interchange`.
