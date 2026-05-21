---
date: 2026-05-04 14:25:10 EDT
repo: git@github.com:jmagar/lab.git
branch: bd-work/mcp-gateway-review-remediation
head: 295c6181
agent: Codex
session id: 019df42e-fdc3-7100-9c1c-a5c5b39a39c6
transcript: /home/jmagar/.codex/sessions/2026/05/04/rollout-2026-05-04T14-10-13-019df42e-fdc3-7100-9c1c-a5c5b39a39c6.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab  295c6181 [bd-work/mcp-gateway-review-remediation]
pr: "#40 Integrate service wave and CI updates https://github.com/jmagar/lab/pull/40"
---

# Codex Plugin Startup Warning Fix

## User Request

The user reported repeated Codex startup warnings for invalid plugin skill YAML and failed MCP startup for `axon` and `lab`, then asked to fix them. After the fix, the user invoked `vibin:save-to-md` to capture the session.

## Session Overview

- Fixed two malformed `SKILL.md` YAML frontmatter blocks in the local labby marketplace plugin cache.
- Diagnosed MCP startup failures for the enabled `axon` and `lab` plugins.
- Changed plugin MCP manifests to launch already-built binaries instead of Cargo wrapper commands.
- Updated the local `axon` and `lab` shell wrappers to prefer built binaries and fall back to Cargo only if the binaries are missing.
- Verified JSON/YAML parsing and MCP `initialize` responses for the edited plugin manifests.

## Sequence of Events

1. Inspected the invalid skill files and confirmed both failures came from unquoted colons in YAML `description:` values.
2. Rewrote those descriptions as YAML folded block scalars.
3. Located the enabled plugin MCP manifests for `axon` and `lab`.
4. Confirmed the manifests launched `axon mcp` and `lab mcp`, and that those commands resolved to shell wrappers.
5. Tested the wrapper behavior and found `lab --help` failed through Cargo with an `rsa` dependency conflict while `axon --help` triggered a build.
6. Tested the built binaries directly and confirmed both responded successfully to line-delimited MCP `initialize`.
7. Patched plugin manifests and local wrappers, then reran parsing and handshake checks.

## Key Findings

- The skipped skills were caused by YAML parsing, not missing files:
  - `/home/jmagar/.codex/plugins/cache/labby-marketplace/lab/local/skills/lab-service-onboarding/SKILL.md`
  - `/home/jmagar/.codex/plugins/cache/labby-marketplace/rust/local/skills/cargo-perf/SKILL.md`
- The enabled plugin manifests were:
  - `/home/jmagar/.codex/plugins/cache/claude-homelab/axon/local/.mcp.json`
  - `/home/jmagar/.codex/plugins/cache/claude-homelab/lab/local/.mcp.json`
  - `/home/jmagar/.codex/plugins/cache/labby-marketplace/lab/local/.mcp.json`
- `/home/jmagar/.local/bin/axon` and `/home/jmagar/.cargo/bin/lab` were Bash wrappers that invoked Cargo on every run.
- `/home/jmagar/workspace/lab/target/debug/labby mcp` and `/home/jmagar/workspace/axon_rust/target/debug/axon mcp` successfully completed MCP `initialize`.

## Technical Decisions

- Used YAML folded block scalars for long skill descriptions to preserve the original text while avoiding YAML colon parsing ambiguity.
- Pointed MCP manifests at concrete built binaries to avoid startup-time Cargo dependency resolution, compilation, and lockfile drift during MCP handshake.
- Kept Cargo fallback behavior in the shell wrappers so normal CLI usage still works if a debug binary is absent.
- Did not edit repo source files because the failures were in plugin cache and host-local wrapper configuration.

## Files Modified

- `/home/jmagar/.codex/plugins/cache/labby-marketplace/lab/local/skills/lab-service-onboarding/SKILL.md` - converted invalid description frontmatter to a folded scalar.
- `/home/jmagar/.codex/plugins/cache/labby-marketplace/rust/local/skills/cargo-perf/SKILL.md` - converted invalid description frontmatter to a folded scalar.
- `/home/jmagar/.codex/plugins/cache/claude-homelab/axon/local/.mcp.json` - changed MCP command to `/home/jmagar/workspace/axon_rust/target/debug/axon`.
- `/home/jmagar/.codex/plugins/cache/claude-homelab/lab/local/.mcp.json` - changed MCP command to `/home/jmagar/workspace/lab/target/debug/labby`.
- `/home/jmagar/.codex/plugins/cache/labby-marketplace/lab/local/.mcp.json` - changed MCP command to `/home/jmagar/workspace/lab/target/debug/labby`.
- `/home/jmagar/.local/bin/axon` - changed wrapper to prefer the built Axon binary before Cargo fallback.
- `/home/jmagar/.cargo/bin/lab` - changed wrapper to prefer the built Labby binary before Cargo fallback.
- `docs/sessions/2026-05-04-codex-plugin-startup-warning-fix.md` - saved this session note.

## Commands Executed

- `sed -n '1,80p' .../SKILL.md` to inspect broken skill frontmatter.
- `rg -n ...` and `find ... -path '*/.mcp.json'` to locate plugin manifests and relevant configuration.
- `command -v axon`, `command -v lab`, and `file` to identify wrapper scripts.
- `lab --help` to reproduce the Cargo dependency-resolution failure.
- `/home/jmagar/workspace/lab/target/debug/labby --help` and `/home/jmagar/workspace/axon_rust/target/debug/axon --help` to verify built binaries.
- Python MCP probes that sent line-delimited `initialize` requests to the configured commands.
- `ps -ef | rg ...` and targeted `kill -TERM` commands to stop build processes spawned by wrapper probes.

## Errors Encountered

- First attempted a small Python edit script with an incorrect assumption about the description line index; it exited without modifying files. The patch was then applied directly with `apply_patch`.
- A broad `rg` over `.codex`, `.config`, `.claude`, and repo docs produced excessive output including credential/cache noise. Subsequent searches were narrowed to plugin cache and manifest paths.
- The initial MCP probe used `Content-Length` framing. Both tested servers expected newline-delimited JSON on stdio for this client path; retrying with line-delimited JSON produced valid `initialize` responses.
- `lab --help` through the wrapper failed because Cargo could not resolve conflicting `rsa` versions between `russh` and `lab-auth`.

## Behavior Changes (Before/After)

| Area | Before | After |
| --- | --- | --- |
| Skill loading | Two labby marketplace skills were skipped for invalid YAML. | The two edited skill frontmatter blocks parse successfully. |
| Axon MCP startup | Plugin launched `axon mcp`, which invoked a Cargo wrapper. | Plugin launches `/home/jmagar/workspace/axon_rust/target/debug/axon mcp` directly. |
| Lab MCP startup | Plugin launched `lab mcp`, which invoked a Cargo wrapper and could fail before handshake. | Plugin launches `/home/jmagar/workspace/lab/target/debug/labby mcp` directly. |
| Local CLI wrappers | `axon` and `lab` always invoked Cargo. | Both wrappers prefer built debug binaries and use Cargo only as fallback. |

## Verification Evidence

| Command | Expected | Actual | Status |
| --- | --- | --- | --- |
| `python3` JSON/YAML parse check | Edited manifests and skill frontmatter parse. | All three `.mcp.json` files parsed; both edited `SKILL.md` files parsed with PyYAML. | Pass |
| Python line-delimited MCP initialize probe for `claude-homelab/axon` manifest | MCP server returns initialize result. | Returned `protocolVersion: 2025-06-18` and server info. | Pass |
| Python line-delimited MCP initialize probe for `claude-homelab/lab` manifest | MCP server returns initialize result. | Returned `protocolVersion: 2025-06-18` and server info. | Pass |
| Python line-delimited MCP initialize probe for `labby-marketplace/lab` manifest | MCP server returns initialize result. | Returned `protocolVersion: 2025-06-18` and server info. | Pass |
| `axon --help` | Return without triggering Cargo. | Printed Axon CLI help immediately. | Pass |
| `lab --help` | Return without triggering Cargo. | Printed Labby CLI help immediately. | Pass |
| `ps -ef | rg ...cargo/rustc...` | No leftover build processes from probes. | Only the `rg` command itself matched. | Pass |

## Risks and Rollback

- These changes are host-local and outside the Git-tracked lab repo, except this ignored session note.
- Plugin cache edits may be overwritten by reinstalling or refreshing the marketplace plugins.
- The manifest paths now depend on the current debug binaries existing:
  - `/home/jmagar/workspace/axon_rust/target/debug/axon`
  - `/home/jmagar/workspace/lab/target/debug/labby`
- Rollback is to restore the three `.mcp.json` command values to `axon` / `lab`, and restore the two wrapper scripts to direct Cargo invocations.

## Decisions Not Taken

- Did not repair the `rsa` dependency conflict in the lab workspace because it was not required to fix MCP startup once manifests avoided Cargo wrappers.
- Did not disable the duplicate enabled lab plugin entries; both configured lab manifests now handshake successfully.
- Did not modify repo `AGENTS.md` or `CLAUDE.md`; the issue was in plugin cache and wrapper configuration.

## Open Questions

- Whether plugin marketplace refreshes should regenerate these fixed skill files and manifests from source rather than relying on cache edits.
- Whether the lab workspace `rsa` version conflict should be fixed separately so Cargo fallback is reliable.

## Next Steps

- Restart the Codex session/client so plugin discovery and MCP startup run from the updated files.
- If startup warnings persist after restart, capture the fresh warning text and inspect whether another enabled plugin cache copy is being loaded.
- If this note needs to be committed later, force-add it because `docs/sessions/` is ignored by this repo.
