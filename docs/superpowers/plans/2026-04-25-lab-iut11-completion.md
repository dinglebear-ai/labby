# lab-iut1.1 StashMeta Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Add the shared marketplace stash metadata and drift cache foundation required by bead `lab-iut1.1` without wiring new artifact fork/diff/patch actions.

**Architecture:** Implement a focused `stash_meta.rs` module under `crates/lab/src/dispatch/marketplace/` containing durable metadata types, atomic JSON I/O helpers, fs4 lock acquisition, base snapshot helpers, path validation, and drift-cache helpers. Expose the module from `marketplace.rs` for later beads while keeping existing `update.rs` action behavior unchanged and compatible.

**Tech Stack:** Rust 2024, serde/serde_json, tempfile, fs4, xxhash-rust, std filesystem APIs, cargo test/clippy.

---

### Task 1: Add dependencies and module exposure

**Files:**
- Modify: `crates/lab/Cargo.toml`
- Modify: `crates/lab/src/dispatch/marketplace.rs`

- [x] **Step 1: Add direct dependencies**

Add `fs4 = "1"` and `xxhash-rust = { version = "0.8", features = ["xxh3"] }` to `crates/lab/Cargo.toml` near existing filesystem/hash dependencies. Do not add `fs2`.

- [x] **Step 2: Expose the shared module**

Add `pub(crate) mod stash_meta;` to `crates/lab/src/dispatch/marketplace.rs` near the other marketplace submodules.

### Task 2: Implement stash metadata types and JSON I/O

**Files:**
- Create: `crates/lab/src/dispatch/marketplace/stash_meta.rs`

- [x] **Step 1: Define public metadata types**

Create `StashMeta`, `ForkType`, `PatchRecord`, `UpdateConfig`, `ConflictStrategy`, `DriftCache`, `DriftCacheEntry`, and `DriftStatus`. Include durable bead fields and a `content_hashes: HashMap<String, String>` field with serde defaulting for compatibility.

- [x] **Step 2: Implement metadata reads**

Implement `read_stash_meta(stash_dir: &Path) -> Result<Option<StashMeta>, ToolError>`. Return `Ok(None)` if `.stash.json` is absent, if `schema_version` is missing, or if `schema_version` is `0`; return `decode_error` for malformed JSON.

- [x] **Step 3: Implement metadata writes**

Implement `write_stash_meta(stash_dir: &Path, meta: &StashMeta) -> Result<(), ToolError>` using `NamedTempFile::new_in(stash_dir)`, `write_all`, `sync_all`, and `persist` for atomic same-directory replacement.

- [x] **Step 4: Implement fs4 locking**

Implement `acquire_stash_lock(stash_dir: &Path) -> Result<File, ToolError>` by creating `stash_dir`, opening `.stash.lock`, and calling `fs4::fs_std::FileExt::lock_exclusive()` on the returned file.

### Task 3: Implement base snapshot and drift cache helpers

**Files:**
- Modify: `crates/lab/src/dispatch/marketplace/stash_meta.rs`

- [x] **Step 1: Implement path validation**

Implement `validate_rel_path(rel_path: &str) -> Result<(), ToolError>` using an empty check plus `Path::new(rel_path).components()` with `Component::Normal(_)` as the only accepted component kind.

- [x] **Step 2: Implement base snapshot helpers**

Implement `read_base_snapshot`, `write_base_snapshot`, `delete_base_snapshot`, and `list_base_snapshots`. `write_base_snapshot` must validate before joining, create parent directories, use `OpenOptions::create_new(true)`, and fall back to truncate-only open; do not use `std::fs::copy`.

- [x] **Step 3: Implement drift cache helpers**

Implement `read_drift_cache`, `write_drift_cache`, `compute_base_hash`, and `check_drift`. Use xxhash3 hex hashes, stat mtime/size before reading, return `Deleted` if the working copy is absent, and return `BaseMissing` if the base snapshot is absent.

### Task 4: Add focused tests

**Files:**
- Modify: `crates/lab/src/dispatch/marketplace/stash_meta.rs`

- [x] **Step 1: Add path validation tests**

Cover accepted `agents/foo.md` and `skills/bar/baz.md`, and rejected `../secrets`, `/etc/passwd`, `a/../b`, null bytes, empty string, and `C:\windows`.

- [x] **Step 2: Add metadata tests**

Cover absent metadata returning `None`, missing/zero schema returning `None`, and roundtrip preservation of all fields including `content_hashes`.

- [x] **Step 3: Add base snapshot tests**

Cover unknown read returning `None`, parent directory creation, listing relative paths, and symlink destination behavior for Unix.

- [x] **Step 4: Add lock and drift tests**

Cover a second lock acquisition waiting until the first file lock is dropped, base hash computation, cached clean/dirty drift, changed-file rehash, deleted working file, and missing base snapshot.

### Task 5: Verify and document the session

**Files:**
- Modify: `docs/superpowers/plans/2026-04-25-lab-iut11-completion.md`
- Create: `docs/sessions/2026-04-25-lab-iut11-completion.md`

- [x] **Step 1: Run focused tests**

Run: `cargo test -p lab --all-features dispatch::marketplace::stash_meta::tests`
Expected: PASS.

- [x] **Step 2: Run relevant marketplace update tests**

Run: `cargo test -p lab --all-features dispatch::marketplace::update::tests`
Expected: PASS.

- [x] **Step 3: Run bead validation**

Run: `cargo test -p lab --all-features`
Expected: PASS.

Run: `cargo clippy -p lab --all-features -- -D warnings`
Expected: PASS.

Run: `rg -n "fs2" .`
Expected: no matches.

Run: `rg -n "std::fs::copy" crates/lab/src/dispatch/marketplace/stash_meta.rs`
Expected: no matches.

Run: `rg -n "unwrap\(" crates/lab/src/dispatch/marketplace/stash_meta.rs`
Expected: no production matches.

- [x] **Step 4: Complete tracking docs**

Mark this plan complete and create the required session report with command evidence, file list, risks, and blockers.
