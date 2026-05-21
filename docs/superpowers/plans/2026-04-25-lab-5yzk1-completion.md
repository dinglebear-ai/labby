# lab-5yzk.1 Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Finish bead `lab-5yzk.1` by ensuring all 17 service CLI shims expose complete clap action possible values, route through the shared dispatch layer, use the correct action field type/default, and honor destructive gates.

**Architecture:** Keep all CLI files as thin shims over `crate::dispatch::<service>::dispatch`. Use the shared `crate::cli::helpers::action_parser(ACTIONS)` helper for clap possible values and `run_confirmable_action_command` for any shim whose dispatch catalog contains destructive actions.

**Tech Stack:** Rust 2024, clap derive, serde_json, shared `lab` dispatch layer, `lab_apis::core::action::ActionSpec`.

---

## File Structure

- `crates/lab/src/cli/helpers.rs`: already contains `action_parser`, `print_dry_run`, `run_action_command`, and `run_confirmable_action_command`; verify the helper remains available.
- `crates/lab/src/cli/arcane.rs`: only remaining incomplete shim; add `action_parser` import and set `#[arg(default_value = "help", value_parser = action_parser(ACTIONS))]` on `ArcaneArgs.action`.
- `crates/lab/src/cli/{apprise,linkding,memos,openai,overseerr,paperless,plex,prowlarr,qbittorrent,qdrant,sabnzbd,sonarr,tailscale,tautulli,tei,unraid}.rs`: already match the migrated pattern; verify they remain compliant.
- `docs/sessions/2026-04-25-lab-5yzk1-completion.md`: session report after implementation and verification.

## Current Findings Before Code Changes

- `action_parser()` already exists in `crates/lab/src/cli/helpers.rs`.
- No target CLI shim currently calls `crate::mcp::services::*::dispatch`.
- No target CLI shim currently uses `action: Option<String>` or the old `unwrap_or_else(|| "help".to_string())` fallback.
- `arcane` is the only target shim missing `action_parser(ACTIONS)` and `default_value = "help"`.
- Destructive-action shims among the 17 currently route through `run_confirmable_action_command`; `arcane` already has `yes` and `dry_run` fields and uses the confirmable helper.

### Task 1: Prove the remaining CLI parser gap exists

**Files:**
- Inspect/Exercise: `crates/lab/src/cli/arcane.rs`

- [x] **Step 1: Run the failing behavior check**

Run: `cargo run -p lab --all-features -- arcane --help`

Expected before the fix: help for `lab arcane` does not show clap `[possible values: ...]` for the `action` positional, because `ArcaneArgs.action` has no `value_parser`.

### Task 2: Fix the arcane action parser/default

**Files:**
- Modify: `crates/lab/src/cli/arcane.rs`

- [x] **Step 1: Import `action_parser` with the existing confirmable helper**

Change:

```rust
use crate::cli::helpers::run_confirmable_action_command;
```

To:

```rust
use crate::cli::helpers::{action_parser, run_confirmable_action_command};
```

- [x] **Step 2: Add the clap parser/default to the action field**

Change:

```rust
/// Action to run, e.g. `help`, `system.health`, `container.list`.
pub action: String,
```

To:

```rust
/// Action to run, e.g. `help`, `system.health`, `container.list`.
#[arg(default_value = "help", value_parser = action_parser(ACTIONS))]
pub action: String,
```

### Task 3: Verify all 17 shims and destructive gates

**Files:**
- Verify: `crates/lab/src/cli/helpers.rs`
- Verify: `crates/lab/src/cli/apprise.rs`
- Verify: `crates/lab/src/cli/arcane.rs`
- Verify: `crates/lab/src/cli/linkding.rs`
- Verify: `crates/lab/src/cli/memos.rs`
- Verify: `crates/lab/src/cli/openai.rs`
- Verify: `crates/lab/src/cli/overseerr.rs`
- Verify: `crates/lab/src/cli/paperless.rs`
- Verify: `crates/lab/src/cli/plex.rs`
- Verify: `crates/lab/src/cli/prowlarr.rs`
- Verify: `crates/lab/src/cli/qbittorrent.rs`
- Verify: `crates/lab/src/cli/qdrant.rs`
- Verify: `crates/lab/src/cli/sabnzbd.rs`
- Verify: `crates/lab/src/cli/sonarr.rs`
- Verify: `crates/lab/src/cli/tailscale.rs`
- Verify: `crates/lab/src/cli/tautulli.rs`
- Verify: `crates/lab/src/cli/tei.rs`
- Verify: `crates/lab/src/cli/unraid.rs`

- [x] **Step 1: Static compliance checks**

Run:

```bash
rg -n 'mcp::services|action: Option<String>|unwrap_or_else\(\|\| "help"|PossibleValuesParser::new\(ACTIONS' crates/lab/src/cli/{apprise,arcane,linkding,memos,openai,overseerr,paperless,plex,prowlarr,qbittorrent,qdrant,sabnzbd,sonarr,tailscale,tautulli,tei,unraid}.rs
```

Expected: no matches.

Run:

```bash
rg -n 'default_value = "help", value_parser = action_parser\(ACTIONS\)' crates/lab/src/cli/{apprise,arcane,linkding,memos,openai,overseerr,paperless,plex,prowlarr,qbittorrent,qdrant,sabnzbd,sonarr,tailscale,tautulli,tei,unraid}.rs
```

Expected: 17 matches.

- [x] **Step 2: Build-level verification**

Run: `cargo check --workspace --all-features`

Expected: command exits 0.

Run: `cargo build --workspace --all-features`

Expected: command exits 0.

- [x] **Step 3: Help possible-values verification for every covered service**

Run:

```bash
for svc in apprise arcane linkding memos openai overseerr paperless plex prowlarr qbittorrent qdrant sabnzbd sonarr tailscale tautulli tei unraid; do
  cargo run -p lab --all-features -- "$svc" --help | grep -q 'possible values:' || exit 1
done
```

Expected: command exits 0.

- [x] **Step 4: Invalid action parser verification**

Run: `cargo run -p lab --all-features -- apprise invalid_action`

Expected: non-zero clap parse error that includes `invalid_action` and valid possible values.

- [x] **Step 5: Destructive gate verification**

Run: `cargo run -p lab --all-features -- arcane volume.delete`

Expected in non-interactive execution: non-zero refusal with `pass -y / --yes to confirm destructive action` before dispatch.

Run: `cargo run -p lab --all-features -- arcane volume.delete -y --dry-run`

Expected: exits 0 and prints `[dry-run] would dispatch arcane action` without calling the service.

### Task 4: Write session report

**Files:**
- Create: `docs/sessions/2026-04-25-lab-5yzk1-completion.md`

- [x] **Step 1: Gather required report context**

Run the user-specified report context commands and include concrete outputs in the report.

- [x] **Step 2: Document the session facts**

Include the required YAML metadata block and the required sections: User Request, Session Overview, Sequence of Events, Key Findings, Technical Decisions, Files Modified, Commands Executed, Errors Encountered if any, Behavior Changes, Verification Evidence, Risks and Rollback, Decisions Not Taken if any, References if any, Open Questions if any, Next Steps.

