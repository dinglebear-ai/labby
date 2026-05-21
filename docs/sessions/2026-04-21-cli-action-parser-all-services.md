# CLI Action Parser — All Services

**Date:** 2026-04-21  
**Branch:** fix/auth  
**Bead:** lab-5yzk.1

---

## Session Overview

All 20 dispatch-backed CLI service shims were missing `PossibleValuesParser` on their `action` argument, so `--help` showed no list of valid actions. This session fixed every shim: added the `action_parser()` helper to `helpers.rs`, then updated all 17 remaining shims (4 were fixed in the prior session). Additionally, 8 shims had a forbidden `cli → mcp` dependency and 15 had destructive actions with no confirmation gate — both were corrected in the same pass.

---

## Timeline

| Time | Activity |
|------|----------|
| Session start | Context restore from compacted prior session |
| Phase 1 | Read all 17 outstanding shim files via Bash + Read tool |
| Phase 2 | Verified `ACTIONS` export and destructive flag status per service |
| Phase 3 | Wrote all 17 updated shim files in parallel batches |
| Phase 4 | `cargo check --workspace --all-features` — 0 errors, 2 pre-existing warnings |
| Phase 5 | Runtime `--help` spot-check for all 20 services — all pass |

---

## Key Findings

- **Root cause**: CLI shims declared `action: Option<String>` with no `value_parser`, so clap had no knowledge of valid values.
- **Correct field shape**: `action: String` + `#[arg(default_value = "help", value_parser = action_parser(ACTIONS))]` (not `Option<String>`).
- **8 shims had `cli → mcp` dependency**: `apprise`, `arcane`, `memos`, `qbittorrent`, `qdrant`, `tailscale`, `tautulli`, `openai` all called `crate::mcp::services::<svc>::dispatch` — forbidden per `src/CLAUDE.md`.
- **15 services have destructive actions** (confirmed via `grep "destructive: true"` on catalogs): `linkding`, `plex`, `prowlarr`, `overseerr`, `paperless`, `sonarr`, `apprise`, `arcane`, `memos`, `qbittorrent`, `qdrant`, `tailscale`, `tautulli`, `sabnzbd`, `unraid` — but only `sabnzbd` and `unraid` previously had `-y/--yes` gates.
- **Pre-existing dead_code warnings**: `mcp/services/tailscale.rs` and `mcp/services/tautulli.rs` `dispatch` functions are never called — these services are not registered in `mcp/registry.rs`. The CLI-to-MCP dependency was masking this gap; my fix surfaced it.

---

## Technical Decisions

1. **`action_parser()` helper** added to `crates/lab/src/cli/helpers.rs:18-20` — avoids repeating the verbose `PossibleValuesParser::new(actions.iter().map(|a| a.name))` expression 17 times; takes `&'static [ActionSpec]`.

2. **All destructive shims get `--dry-run`** — `cli/CLAUDE.md` requires `--dry-run` for destructive services; simpler `println!` pattern (matching `mcpregistry.rs`) rather than the `OutputFormat` match in old `sabnzbd`.

3. **`sabnzbd` dry_run simplified** — original had an `OutputFormat::Json` branch emitting structured JSON for dry-run; replaced with plain `println!` to match reference pattern and remove the special-case branch.

4. **`unraid --dry-run` added** — `--instance` injection logic preserved; dry-run check placed after instance merge so the printed params include the injected instance.

5. **`openai` dispatch fixed** — was calling `crate::mcp::services::openai::dispatch`; switched to `crate::dispatch::openai::dispatch`. `openai` has no destructive actions so stays with `run_action_command`.

---

## Files Modified

| File | Change |
|------|--------|
| `crates/lab/src/cli/helpers.rs` | Added `action_parser()` helper (prior session) |
| `crates/lab/src/cli/tei.rs` | Field type fix + `action_parser`; safe, no destructive actions |
| `crates/lab/src/cli/openai.rs` | Field type fix + `action_parser` + fix mcp→dispatch |
| `crates/lab/src/cli/linkding.rs` | Field type fix + `action_parser` + `run_confirmable` + `-y`/`--dry-run` |
| `crates/lab/src/cli/plex.rs` | Field type fix + `action_parser` + `run_confirmable` + `-y`/`--dry-run` |
| `crates/lab/src/cli/prowlarr.rs` | Field type fix + `action_parser` + `run_confirmable` + `-y`/`--dry-run` |
| `crates/lab/src/cli/overseerr.rs` | Field type fix + `action_parser` + `run_confirmable` + `-y`/`--dry-run` |
| `crates/lab/src/cli/paperless.rs` | Field type fix + `action_parser` + `run_confirmable` + `-y`/`--dry-run` |
| `crates/lab/src/cli/sonarr.rs` | Field type fix + `action_parser` + `run_confirmable` + `-y`/`--dry-run` |
| `crates/lab/src/cli/apprise.rs` | Field type fix + `action_parser` + fix mcp→dispatch + `run_confirmable` + `-y`/`--dry-run` |
| `crates/lab/src/cli/arcane.rs` | Field type fix + `action_parser` + fix mcp→dispatch + `run_confirmable` + `-y`/`--dry-run` |
| `crates/lab/src/cli/memos.rs` | Field type fix + `action_parser` + fix mcp→dispatch + `run_confirmable` + `-y`/`--dry-run` |
| `crates/lab/src/cli/qbittorrent.rs` | Field type fix + `action_parser` + fix mcp→dispatch + `run_confirmable` + `-y`/`--dry-run` |
| `crates/lab/src/cli/qdrant.rs` | Field type fix + `action_parser` + fix mcp→dispatch + `run_confirmable` + `-y`/`--dry-run` |
| `crates/lab/src/cli/tailscale.rs` | Field type fix + `action_parser` + fix mcp→dispatch + `run_confirmable` + `-y`/`--dry-run` |
| `crates/lab/src/cli/tautulli.rs` | Field type fix + `action_parser` + fix mcp→dispatch + `run_confirmable` + `-y`/`--dry-run` |
| `crates/lab/src/cli/sabnzbd.rs` | Field type fix + `action_parser` + simplify `dry_run` branch |
| `crates/lab/src/cli/unraid.rs` | Field type fix + `action_parser` + `--dry-run` added |

**Not modified** (already correct from prior session): `gotify.rs`, `bytestash.rs`, `unifi.rs`, `mcpregistry.rs`

---

## Commands Executed

```bash
# Verified destructive flags per service
grep -c "destructive: true" /path/to/dispatch/<svc>/catalog.rs

# Verified ACTIONS re-exports in dispatch entry points
grep -r "pub use.*ACTIONS|pub const ACTIONS" /path/to/dispatch/<svc>.rs

# Compile check
cargo check --workspace --all-features
# Result: 0 errors, 2 warnings (pre-existing dead_code in mcp/services/tailscale.rs + tautulli.rs)

# Runtime verification — all 20 services
for svc in apprise arcane bytestash gotify linkding memos openai overseerr paperless plex prowlarr \
           qbittorrent qdrant sabnzbd sonarr tailscale tautulli tei unifi unraid; do
  ./target/debug/lab $svc --help | grep "possible values:"
done
```

---

## Behavior Changes (Before / After)

| Service | Before | After |
|---------|--------|-------|
| All 17 shims | `<ACTION>` — no valid values shown in `--help` | `[possible values: help, schema, ...]` listed |
| `apprise`, `arcane`, `memos`, `qbittorrent`, `qdrant`, `tailscale`, `tautulli` | Called `crate::mcp::services::<svc>::dispatch` (forbidden) | Calls `crate::dispatch::<svc>::dispatch` |
| `linkding`, `plex`, `prowlarr`, `overseerr`, `paperless`, `sonarr` | No `-y` / `--dry-run`; destructive actions ran without confirmation | `-y/--yes`, `--no-confirm`, `--dry-run` enforced |
| `sabnzbd` | `dry_run` had `OutputFormat` match with JSON branch | Simple `println!` pattern matching reference |
| `unraid` | No `--dry-run` flag | `--dry-run` added |
| `openai` | Routed through MCP dispatch | Routes through shared dispatch layer |

---

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `cargo check --workspace --all-features` | 0 errors | 0 errors, 2 warnings | ✅ |
| `lab gotify --help \| grep "possible values"` | Full action list | 26 actions listed | ✅ |
| `lab sonarr --help \| grep "possible values"` | Full action list | 35 actions listed | ✅ |
| `lab apprise --help \| grep "possible values"` | Full action list | 10 actions listed | ✅ |
| `lab tei --help \| grep "possible values"` | Full action list | 10 actions listed | ✅ |
| `lab unraid --help \| grep "possible values"` | Full action list | 31 actions listed | ✅ |
| All 20 services `--help` | `possible values:` present | All 20 show values | ✅ |

---

## Risks and Rollback

- **Low risk**: Changes are additive (new flags) or behaviorally equivalent (dispatch path fix routes to same underlying logic).
- **Destructive gate enforcement**: 13 services now require `-y` for destructive actions where they didn't before. This is a breaking change for any scripts calling these services without `-y`. Rollback: revert the 13 affected shim files.
- **`--dry-run` on unraid**: Placement after `--instance` merge is intentional — prints the fully-merged params object.
- **Rollback path**: `git checkout crates/lab/src/cli/{apprise,arcane,linkding,memos,openai,overseerr,paperless,plex,prowlarr,qbittorrent,qdrant,sabnzbd,sonarr,tailscale,tautulli,tei,unraid}.rs`

---

## Decisions Not Taken

- **`Option<String>` with `value_parser`**: Applying `PossibleValuesParser` to `Option<String>` is invalid — clap requires `String` when a `PossibleValuesParser` is set. The `default_value = "help"` + `String` pattern is the correct shape.
- **Per-service `possible_values` const arrays**: Rejected in favor of the `action_parser(ACTIONS)` helper that derives directly from the catalog — no duplication.
- **Suppressing dead_code warnings for mcp/services/tailscale + tautulli**: Rejected — the warnings correctly surface that these services are missing MCP registry registration.

---

## Open Questions

- `mcp/services/tailscale.rs` and `mcp/services/tautulli.rs` dispatch functions are dead code — these services are not registered in `mcp/registry.rs`. Should they be added to the registry?
- `sabnzbd` params default changed from `serde_json::Value::Null` to `serde_json::Value::Object(...)` — verify this doesn't affect any callers expecting null.

---

## Next Steps

- Register `tailscale` and `tautulli` in `mcp/registry.rs` (pre-existing gap now surfaced).
- Consider upgrading `linkding`, `plex`, `prowlarr`, and other dispatch-backed shims to typed `Subcommand` enums (Tier 1) if richer UX is needed.
- Close bead `lab-5yzk.1` once PR is merged.
