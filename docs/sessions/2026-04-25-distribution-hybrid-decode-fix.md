---
date: 2026-04-25 15:52:01 EST
repo: git@github.com:jmagar/lab.git
branch: bd-security/marketplace-p1-fixes
head: f168964b
agent: Claude (claude-sonnet-4-6)
session id: 9fe013aa-887c-4819-b052-0fe71b2f6fc1
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/9fe013aa-887c-4819-b052-0fe71b2f6fc1.jsonl
working directory: /home/jmagar/workspace/lab
pr: "#29 — fix(marketplace): P1 security fixes — path traversal, symlink following, installPath validation — https://github.com/jmagar/lab/pull/29"
---

## User Request

Debug a `decode_error` surfaced by the `marketplace` service `agent.list` action:

```
15:28:40   WARN  decode error: error decoding response body  kind=decode_error  method=GET  elapsed_ms=122  host=cdn.agentclientprotocol.com  path=/registry/v1/latest/registry.json
15:28:40  ERROR  dispatch error  kind=decode_error  request_id=270fbdaa-ff27-4b1c-bedc-c482a6adf5a2  elapsed_ms=128  action=agent.list  service=marketplace  surface=api
```

## Session Overview

Traced a `decode_error` from `agent.list` to a serde schema mismatch in the ACP registry client. The real CDN returns agents with **hybrid distributions** (multiple method keys in one `distribution` object), which serde could not decode into the existing externally-tagged enum. Fixed by changing `Distribution` from an enum to a struct, updated all dispatch match arms, and added a regression test.

## Sequence of Events

1. Applied the `superpowers:systematic-debugging` skill to structure the investigation.
2. Read `crates/lab-apis/src/acp_registry/client.rs` and `types.rs` to understand what type the registry JSON is decoded into.
3. Fetched the live registry at `https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json` via a research subagent to observe the actual response shape.
4. Identified the mismatch: some registry agents expose both `binary` and `npx` keys in one `distribution` object; the Rust `Distribution` enum requires exactly one variant key.
5. Searched for all `Distribution::` pattern-match sites to bound the change scope — found two match blocks in `acp_dispatch.rs`.
6. Rewrote `Distribution` as a struct with optional fields and an `extra` catchall.
7. Added `args: Vec<String>` to `BinaryAsset` (real registry has this field; was previously silently dropped).
8. Updated `install_local` and `install_remote` in `acp_dispatch.rs` to use field access with priority ordering (binary → npx → uvx).
9. Added regression test `test_hybrid_distribution_decodes` in `client.rs`.
10. Ran `cargo test -p lab-apis --all-features acp_registry` — 9 tests passed.

## Key Findings

- `crates/lab-apis/src/acp_registry/types.rs:62-73`: `Distribution` was `#[serde(rename_all = "snake_case")] enum` — serde's externally-tagged repr requires a single-key JSON object.
- The live registry at `cdn.agentclientprotocol.com` returns e.g. `{"distribution": {"binary": {...}, "npx": {...}}}` for at least one agent ("codex-acp" variant).
- `crates/lab/src/dispatch/marketplace/acp_dispatch.rs:217-270`: Two `match &agent.distribution { Distribution::Binary(...) => ... }` blocks — both needed updating.
- `BinaryAsset` was missing `args: Vec<String>` — live registry populates it (e.g. `["acp"]`, `["--acp"]`) but serde silently dropped it without `deny_unknown_fields`. Not the root cause of the error, but a data-loss gap.
- The `stash_meta.rs:125` pre-existing error (`no method named lock_exclusive`) is an unrelated untracked file on this branch — not caused by this session's changes.

## Technical Decisions

**Enum → struct for `Distribution`:** An enum requires exactly one matching variant key. Changing to a struct with optional fields allows any subset of `{binary, npx, uvx}` to be present simultaneously, and `#[serde(flatten)] pub extra: HashMap<String, Value>` handles future unknown distribution types without a decode failure.

**Priority ordering in `install_local` (binary > npx > uvx):** Binary is the most specific (platform-resolved) and already had an early-return path. Keeping that behavior and falling through to npx/uvx preserves backward compatibility for agents that only ship one method.

**Added `not_supported` error kind:** When `agent.distribution` has none of the three known methods, we now return `sdk_kind: "not_supported"` instead of falling off the end of a match. This was previously impossible with an exhaustive enum but is now reachable.

## Files Modified

| File | Purpose |
|------|---------|
| `crates/lab-apis/src/acp_registry/types.rs` | Changed `Distribution` enum → struct; added `BinaryAsset.args` |
| `crates/lab/src/dispatch/marketplace/acp_dispatch.rs` | Replaced enum match arms with struct field access; removed `Distribution` import |
| `crates/lab-apis/src/acp_registry/client.rs` | Added `test_hybrid_distribution_decodes` regression test |

## Commands Executed

```bash
# Confirm lab-apis compiles cleanly after the type change
rtk cargo check -p lab-apis --all-features
# → cargo build (1 crates compiled)

# Confirm workspace compiles (pre-existing stash_meta.rs error unrelated)
rtk cargo check --workspace --all-features
# → 1 errors, 1 warnings (stash_meta.rs — pre-existing untracked file)

# Verify all 9 acp_registry tests pass (8 existing + 1 new regression)
rtk cargo test -p lab-apis --all-features acp_registry
# → 9 passed, 336 filtered out (21 suites, 0.04s)
```

## Errors Encountered

**Pre-existing compile error in `stash_meta.rs:125`:** `no method named lock_exclusive found for std::fs::File`. Root cause is `use fs4::FileExt` not bringing the trait into scope (or `fs4` dep config). This file is untracked on the branch and was present before this session's changes. No action taken; not related to the decode error.

## Behavior Changes (Before / After)

| Aspect | Before | After |
|--------|--------|-------|
| `agent.list` for hybrid-distribution agents | `decode_error` — entire list call fails | Decodes correctly; both distribution methods accessible |
| `agent.install` on hybrid agents | Unreachable (never decoded) | Binary preferred over npx/uvx when available for platform |
| `BinaryAsset.args` | Silently dropped on decode | Captured and available for future use |
| Unknown distribution method keys | Decode failure | Captured in `Distribution.extra: HashMap<String, Value>` |

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `cargo check -p lab-apis --all-features` | No errors | `1 crates compiled` | ✅ |
| `cargo test -p lab-apis --all-features acp_registry` | All pass | `9 passed, 336 filtered out` | ✅ |
| `test_hybrid_distribution_decodes` (new) | Pass | Pass | ✅ |

## Risks and Rollback

- **Low risk.** The change is additive: single-method agents (the majority) continue to deserialize identically — a JSON object `{"npx": {...}}` maps to `Distribution { npx: Some(...), binary: None, uvx: None, extra: {} }`.
- **Rollback:** Revert `types.rs` and `acp_dispatch.rs` to enum-based form. The three pattern-match arms are preserved in git history.
- **Known gap:** The `stash_meta.rs` file is untracked and has a compile error — it will prevent a clean all-features workspace build until addressed separately.

## Decisions Not Taken

**`#[serde(untagged)]` enum:** An untagged enum would try each variant in order and pick the first that succeeds, which is ambiguous for hybrid objects and produces cryptic errors on mismatch. Struct-with-optionals is unambiguous.

**Caller-selected distribution via `distribution_type` param:** Adding a `distribution_type` param to `agent.install` would let callers override the priority. Deferred — not needed to fix the decode error, and the existing behavior (best available method) is sensible.

## References

- Live registry: `https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json`
- Serde externally-tagged enum docs: https://serde.rs/enum-representations.html
- PR #29: https://github.com/jmagar/lab/pull/29

## Next Steps

**Unfinished (started but not completed):**
- None. The decode fix is complete and all tests pass.

**Follow-on tasks not yet started:**
- Resolve the pre-existing `stash_meta.rs` compile error (`lock_exclusive` method missing — likely `fs4::FileExt` import or dep config issue).
- Consider surfacing `Distribution.extra` keys in the `agent.list` response so the frontend can display future distribution types.
- Add `args` to binary install path: `install_binary` currently ignores `BinaryAsset.args`; the field is now decoded but not used.
