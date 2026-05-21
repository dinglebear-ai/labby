---
date: 2026-05-14 16:11:28 EST
repo: git@github.com:jmagar/lab.git
branch: bd-work/lab-n7se-tool-search-review
head: 2d2b95ae
plan: none
agent: Claude (claude-sonnet-4-6)
session id: 2783b421-b2da-461b-8bb5-96e4c47e0b37
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/2783b421-b2da-461b-8bb5-96e4c47e0b37.jsonl
working directory: /home/jmagar/workspace/lab
pr: "#60 — feat(gateway): MCP config discovery and import from external editors — https://github.com/jmagar/lab/pull/60"
---

## User Request

Clone `openclaw/mcporter`, investigate how it auto-discovers MCP configs on the machine, and implement the same patterns in lab to automatically import discovered servers into the gateway — with imported servers disabled by default and provenance tracking (source client + file path).

## Session Overview

Designed, implemented, reviewed, and shipped a complete MCP config discovery and import system for the lab gateway. Added `gateway.discover` (read-only scan of 8 editor/tool config locations) and `gateway.import` (write discovered servers as disabled-by-default entries with provenance). Followed the full lavra pipeline: plan → research → eng-review → lavra-work with 5 waves of parallel agent execution, 3 rounds of review findings, and 18 commits. Tests grew from 2720 to 2794 (+74 new tests).

## Sequence of Events

1. Cloned `openclaw/mcporter` to `~/workspace/mcporter`; ran an Explore agent to map the full discovery architecture (8 client kinds, per-kind path lists, JSON/TOML parsing, dedup, source tracking)
2. Read lab's `UpstreamConfig`, `GatewayManager.add()`, dispatch patterns, and `GatewayConfigView` to understand integration points
3. Called advisor; entered plan mode; user chose two-action split (`gateway.discover` / `gateway.import`) matching the `extract.scan`/`extract.apply` precedent
4. Executed lavra-plan to write `federated-stargazing-swan.md`; user approved
5. Implemented the initial discovery system: `ImportSource`, `env`/`imported_from` on `UpstreamConfig`, `dirs` dep, discovery module with 8 per-client scanners, dispatch handlers, catalog entries, CLI shims, `current_config()` on manager — all committed as `e6e61a77`
6. Ran `gateway discover` smoke test; confirmed 15 servers discovered across codex, windsurf, vscode, gemini
7. Opened PR #60
8. Ran `/pr-review-toolkit:review-pr` → 17 distinct issues found; created beads lab-cj2n through lab-lii0 + lab-hrpl, lab-hw0v, lab-ane5, lab-wmlv, lab-q3ti
9. Created epic lab-qxl8 with all 17 as children; ran lavra-research (4 agents) and lavra-eng-review (4 agents) → added 2 more beads (lab-gofz security, lab-xwql perf) and revised all bead descriptions
10. Applied all eng-review recommendations to bead descriptions; closed lab-277c as YAGNI
11. Ran lavra-work lab-qxl8 → lavra-work-multi → 5 execution waves with parallel agents:
    - Wave 1: lab-gofz, lab-mr78, lab-q3ti, lab-y931
    - Wave 1 lavra-review → 5 introduced findings (lab-qxl8.1–5) + 2 pre-existing (lab-jowd, lab-wsed) — all addressed immediately
    - Wave 2: lab-wmlv, lab-6cdy, lab-k2e9, lab-bb4w, lab-hrpl, lab-km3d
    - Wave 3: lab-lii0, lab-hw0v
    - Wave 4: lab-ane5, lab-cj2n, lab-2dv2, lab-enjz
    - Wave 5: lab-lto2, lab-xwql
12. Fixed merge conflict in dispatch.rs (`McpClientTransportType` test import)
13. Pushed all 18 commits to existing PR #60; closed epic lab-qxl8

## Key Findings

- **mcporter path coverage**: 8 client kinds with platform-specific paths; opencode uses `mcp` key only (no `mcpServers`); codex uses TOML with `mcp_servers`; all others use `mcpServers` → `servers` → `mcp` → root fallback chain
- **`is_control()` is insufficient**: Rust's `char::is_control()` covers only Unicode Cc (C0/C1/DEL) and misses bidi overrides (U+202A–U+202E) that enable Trojan-source display attacks — replaced with ASCII allowlist in `gateway/config.rs`
- **Validation placement bypass**: Name validation in `validate_upstream()` (gateway write-path) didn't run on TOML load path — moved to `UpstreamConfig::validate()` in `config.rs:629`
- **`handle_import` partial state**: Serial `manager.add()` loop aborted on first non-Conflict error leaving partial state — replaced with `batch_add()` that persists once and returns structured `ImportResultView`
- **`home_dir()` test seam**: `handle_discover`/`handle_import` called `discovery::home_dir()` directly with no injection point — extracted `shape_discovered_views()` helper to make view shaping unit-testable
- **`error.kind` vs `kind` taxonomy**: IO error branch in `read_json` used `error.kind = %e.kind()` (dotted field, non-taxonomy value) instead of `kind = "io_error"` per OBSERVABILITY.md

## Technical Decisions

- **Two actions, not one with `dry_run`**: Matches `extract.scan`/`extract.apply` precedent; `gateway.discover` is non-destructive, `gateway.import` is destructive (elicitation fires)
- **`enabled: false` unconditionally**: All imported servers start disabled; user must opt-in via `gateway.update` — no auto-enabling on import
- **Env values not stored**: `UpstreamConfig.env` added but discovery intentionally sets it empty; `env_key_count` on `DiscoveredServer` carries the count for display. Confirmed by linter-added test `imported_upstream_does_not_copy_raw_env_values`
- **ASCII allowlist over `is_control()` blacklist**: Positive allowlist (`is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')`) eliminates entire class of Unicode bypass attacks rather than enumerating dangerous chars
- **`ImportSkipReason` as closed enum**: `AlreadyConfigured` | `Conflict` — not a free-form String; prevents undocumented drift in reason values
- **`GatewayImportParams` stays flat struct**: Codebase uses flat structs with runtime validation (see `GatewayTestParams`); tagged enum would be a novel pattern requiring schema change — added missing "both-provided" guard instead
- **`ClientKind` replaced by inline `KNOWN_CLIENTS` slice**: Proposed enum in `config.rs` would create inverted dependency (config layer knowing about dispatch filter vocabulary); 2-line inline validation in dispatch.rs was simpler

## Files Modified

| File | Purpose |
|------|---------|
| `crates/lab/src/config.rs` | Added `ImportSource` struct, `env`/`imported_from` fields on `UpstreamConfig`, `ImportSource::new()`/`now()` constructors, name validation in `UpstreamConfig::validate()`, `ConfigError::InvalidName` variant |
| `crates/lab/src/dispatch/gateway/discovery.rs` | New module: JSONC stripper, `read_json`, `entry_to_upstream`, `extract_mcp_entries`, `scan_paths`, `discover_all`, `DiscoveredServer`, `env_key_count`, trace logs, 30+ tests |
| `crates/lab/src/dispatch/gateway/discovery/cursor.rs` | Cursor MCP config scanner |
| `crates/lab/src/dispatch/gateway/discovery/claude_code.rs` | Claude Code scanner (strict vs root-fallback split by filename) + tests |
| `crates/lab/src/dispatch/gateway/discovery/claude_desktop.rs` | Claude Desktop scanner |
| `crates/lab/src/dispatch/gateway/discovery/codex.rs` | Codex TOML scanner + observability |
| `crates/lab/src/dispatch/gateway/discovery/gemini.rs` | Gemini CLI scanner |
| `crates/lab/src/dispatch/gateway/discovery/opencode.rs` | OpenCode scanner + env_non_empty fix + tests |
| `crates/lab/src/dispatch/gateway/discovery/vscode.rs` | VS Code / Antigravity / Copilot scanner |
| `crates/lab/src/dispatch/gateway/discovery/windsurf.rs` | Windsurf scanner |
| `crates/lab/src/dispatch/gateway/dispatch.rs` | `handle_discover`, `handle_import` (ImportResultView), `shape_discovered_views` helper, `KNOWN_CLIENTS` validation, `spawn_blocking`, `redact_url_preview`, tests |
| `crates/lab/src/dispatch/gateway/types.rs` | `DiscoveredServerView` (transport→enum), `ImportResultView`, `ImportSkipView`, `ImportSkipReason`, `ImportErrorView`, `McpClientTransportType` +PartialEq/Eq |
| `crates/lab/src/dispatch/gateway/params.rs` | `GatewayDiscoverParams`, `GatewayImportParams`, both-provided guard |
| `crates/lab/src/dispatch/gateway/catalog.rs` | `gateway.discover` (non-destructive) + `gateway.import` (destructive) action specs, updated returns type |
| `crates/lab/src/dispatch/gateway/config.rs` | `validate_upstream`: length guard, ASCII allowlist (replacing `is_control()`), tests; removed duplicated name checks (moved to struct-level) |
| `crates/lab/src/dispatch/gateway/manager.rs` | `current_config()`, `batch_add()`, moved `tracing::info!` to after validation |
| `crates/lab/src/dispatch/gateway/projection.rs` | `imported_from` field in `config_view()` |
| `crates/lab/src/dispatch/gateway.rs` | `pub mod discovery;` declaration |
| `crates/lab/src/cli/gateway.rs` | `GatewayDiscoverArgs`, `GatewayImportArgs`, `Discover`/`Import` enum variants, CLI shims |
| `crates/lab/Cargo.toml` | Added `dirs = "5"` dependency |

## Commands Executed

```bash
# Discovery smoke test
cargo run --all-features -- gateway discover --json
# → 15 servers discovered (codex:2, gemini:7, vscode:1, windsurf:5)

# Client filter test  
cargo run --all-features -- gateway discover --clients gemini
# → 7 gemini servers

# Full test suite (final)
cargo nextest run --all-features -p labby --no-fail-fast
# → 2794 tests run: 2794 passed, 0 skipped
```

## Errors Encountered

- **OOM-killed builds (exit 137)**: Multiple `cargo build` calls were killed by the OS. Switched to `cargo check` + `cargo nextest` separately to avoid full linking overhead.
- **NEVERHANG circuit open**: After 3 OOM kills, zsh-tool NEVERHANG circuit opened. Reset via `zsh_neverhang_reset`.
- **Git merge conflict in dispatch.rs**: Two agents modified `shape_discovered_views` parameter type simultaneously (`GatewayDiscoverParams` vs `super::params::GatewayDiscoverParams`). Resolved by keeping the shorter qualified form. Additionally, test assertions used `assert_eq!(views[0].transport, "http")` after the field became `McpClientTransportType` — fixed by adding `use crate::dispatch::gateway::types::McpClientTransportType` import and switching to `matches!()`.
- **`lab-hw0v` agent stalled**: Returned partial output mid-execution. Retried with a fresh agent prompt that read the files first.

## Behavior Changes (Before/After)

| Area | Before | After |
|------|--------|-------|
| MCP server discovery | Manual `gateway.add` only | `gateway.discover` scans 8 client configs; `gateway.import` batch-imports as disabled |
| Gateway name validation | Only empty-string check | ASCII allowlist + 128-char limit; enforced on both TOML load and write paths |
| `gateway.import` response | `Vec<GatewayView>` (silently dropped skips) | `ImportResultView { imported, skipped, errors }` with closed-reason enum |
| `read_json` error handling | Both ENOENT and parse errors silent | ENOENT silent; IO errors → debug log; parse errors → warn log (named-field syntax) |
| Transport field | `DiscoveredServerView.transport: String` | `transport: McpClientTransportType` (type-safe enum) |
| Import loop | N serial `manager.add()` → N disk writes | Single `batch_add()` → 1 disk write regardless of N |
| `discover_all()` on async thread | Blocking filesystem I/O on Tokio worker | Wrapped in `spawn_blocking` |

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `cargo nextest run --all-features -p labby --no-fail-fast` | 2794 passed | 2794 passed, 0 skipped | ✅ |
| `gateway discover --json \| jq length` | >0 servers | 15 servers | ✅ |
| `gateway discover --clients gemini \| jq length` | 7 | 7 | ✅ |
| `cargo check --all-features -p labby` | 0 errors | 0 errors | ✅ |

## Risks and Rollback

- **Wire-breaking change**: `gateway.import` return type changed from `Vec<GatewayView>` to `ImportResultView`. Any client parsing the old array shape will break. Verified no hardcoded parsing in `apps/gateway-admin/`.
- **ASCII name allowlist is stricter than before**: Names with spaces, slashes, or non-ASCII chars (previously allowed after the `is_control()` era) now fail. Existing configs with such names will fail `LabConfig::validate()` on load. Risk is low (no real MCP server uses those chars).
- **Rollback**: `git revert e6e61a77..HEAD` or `git reset --hard e6e61a77` on the branch. PR #60 can be closed without merging.

## Decisions Not Taken

- **Tagged enum for `GatewayImportParams`** (`ImportTarget::All | ImportTarget::Named { names }`) — rejected because all other params structs in the codebase are flat structs with runtime validation; flat struct + guard matches `GatewayTestParams` precedent exactly
- **`ClientKind` enum in `config.rs`** — rejected because `config.rs` owns persisted types; a dispatch-filter vocabulary enum doesn't belong there. Replaced with 2-line `KNOWN_CLIENTS` inline validation
- **`jiff::Timestamp` for `ImportSource.imported_at`** — deferred because `toml` crate may emit a native datetime literal instead of a quoted string, breaking existing configs. Constructors added (`ImportSource::new`/`now`) using `String` for now
- **`DiscoveredServer` field de-duplication** (lab-277c) — removed as YAGNI; internal-only type with zero correctness impact
- **TTL cache for `discover_all()`** — deferred; `batch_add()` already makes import fast, and the discover+import roundtrip is typically <1s locally

## References

- `~/workspace/mcporter/src/config/imports/paths.ts` — path construction per client kind
- `~/workspace/mcporter/src/config/imports/external.ts` — JSON/TOML parsing logic
- `docs/dev/OBSERVABILITY.md` — tracing field naming conventions
- `docs/dev/ERRORS.md` — error taxonomy and kind stability rules
- PR #60: https://github.com/jmagar/lab/pull/60

## Open Questions

- TOML round-trip for `jiff::Timestamp` as `ImportSource.imported_at`: does `toml::to_string` emit a quoted RFC 3339 string or a native TOML datetime literal? Needs a round-trip test before `lab-6cdy` can be completed fully.
- Should `gateway.discover` be cacheable (short TTL) to avoid double-scan when called immediately before `gateway.import`? Deferred as low-urgency.

## Next Steps

**Unfinished (deferred by design):**
- `lab-6cdy` partial: constructors done; `imported_from.imported_at: String → jiff::Timestamp` field type change needs TOML round-trip verification first

**Follow-on tasks:**
- Address PR #60 review feedback once CI runs
- Consider `gateway discover --include-existing` flag UX in the gateway-admin UI (discovery results could be shown in the servers panel)
- `lab-wsed` and `lab-jowd` are pre-existing issues (filed as standalone beads); schedule for a future cleanup pass
