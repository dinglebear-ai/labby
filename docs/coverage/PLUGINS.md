# Plugin Coverage

All current plugins in `plugins/` with their registered components. Each plugin lives at `plugins/<name>/` and declares itself via `.claude-plugin/plugin.json` and/or `.codex-plugin/plugin.json`.

Plugin manifests intentionally omit `version`; marketplace release identity is Git-SHA based unless an individual plugin explicitly documents a different manifest-level version contract.

**Categories:** agents · bin · commands · hooks · monitors · output-styles · scripts · skills · themes · .mcp.json · .lsp.json · settings.json

---

## acp

| Type | Detail |
|------|--------|
| manifest | `.claude-plugin/plugin.json` |
| manifest | `.codex-plugin/plugin.json` |
| skill | `skills/rust/SKILL.md` |
| .mcp.json | *(empty — no servers registered)* |
| README.md | ✓ |
| CHANGELOG.md | ✓ |

---

## adguard

| Type | Detail |
|------|--------|
| manifest | `.claude-plugin/plugin.json` |
| manifest | `.codex-plugin/plugin.json` |
| skill | `skills/adguard/SKILL.md` |
| README.md | ✓ |
| CHANGELOG.md | ✓ |

---

## agent-os

| Type | Detail |
|------|--------|
| manifest | `.claude-plugin/plugin.json` |
| manifest | `.codex-plugin/plugin.json` |
| command | `commands/agent-os.md` |
| hook | `hooks/hooks.json` |
| script | `scripts/setup.sh` |
| skill | `skills/agent-os/SKILL.md` |
| .mcp.json | `windows-mcp` -> `${user_config.agent_os_mcp_url}` |
| README.md | ✓ |
| CHANGELOG.md | ✓ |

---

## arrs

| Type | Detail |
|------|--------|
| manifest | `.claude-plugin/plugin.json` |
| manifest | `.codex-plugin/plugin.json` |
| hook | `hooks/hooks.json` |
| script | `scripts/setup.sh` |
| skill | `skills/jellyfin/SKILL.md` |
| skill | `skills/overseerr/SKILL.md` |
| skill | `skills/plex/SKILL.md` |
| skill | `skills/prowlarr/SKILL.md` |
| skill | `skills/qbittorrent/SKILL.md` |
| skill | `skills/radarr/SKILL.md` |
| skill | `skills/sabnzbd/SKILL.md` |
| skill | `skills/sonarr/SKILL.md` |
| skill | `skills/tautulli/SKILL.md` |
| skill | `skills/tracearr/SKILL.md` |
| README.md | ✓ |
| CHANGELOG.md | ✓ |

---

## bitwarden

| Type | Detail |
|------|--------|
| manifest | `.claude-plugin/plugin.json` |
| manifest | `.codex-plugin/plugin.json` |
| bin | `bin/bitwarden-mcp` |
| command | `commands/bw-generate.md` |
| command | `commands/bw-get.md` |
| command | `commands/bw-list.md` |
| script | `scripts/install-shell-wrappers` |
| script | `scripts/session` |
| skill | `skills/bitwarden/SKILL.md` |
| .mcp.json | `bitwarden` -> `${CLAUDE_PLUGIN_ROOT}/bin/bitwarden-mcp` |
| README.md | ✓ |
| CHANGELOG.md | ✓ |

---

## broadcastr

| Type | Detail |
|------|--------|
| manifest | `.claude-plugin/plugin.json` |
| manifest | `.codex-plugin/plugin.json` |
| bin | `bin/broadcastr` |
| command | `commands/broadcastr.md` |
| hook | `hooks/hooks.json` |
| monitor | `monitors/monitors.json` |
| script | `scripts/alert-gateway.sh` |
| script | `scripts/emit-fallback.sh` |
| script | `scripts/emit.sh` |
| script | `scripts/format-line.jq` |
| script | `scripts/hook-classify-bash.sh` |
| script | `scripts/hook-on-session-start.sh` |
| script | `scripts/hook-on-stop.sh` |
| script | `scripts/lib-jq-guard.sh` |
| script | `scripts/push-wrapper.sh` |
| script | `scripts/supervisor.sh` |
| script | `scripts/tail-bus.sh` |
| script | `scripts/watch-plans.sh` |
| script | `scripts/watch-sessions.sh` |
| skill | `skills/broadcastr/SKILL.md` |
| skill | `skills/broadcastr-install-hooks/SKILL.md` |
| README.md | ✓ |
| CHANGELOG.md | ✓ |

---

## bytestash

| Type | Detail |
|------|--------|
| manifest | `.claude-plugin/plugin.json` |
| manifest | `.codex-plugin/plugin.json` |
| skill | `skills/bytestash/SKILL.md` |
| README.md | ✓ |
| CHANGELOG.md | ✓ |

---

## dozzle

| Type | Detail |
|------|--------|
| manifest | `.claude-plugin/plugin.json` |
| manifest | `.codex-plugin/plugin.json` |
| skill | `skills/dozzle/SKILL.md` |
| .mcp.json | `dozzle` -> `${userConfig.dozzle_mcp_url}` |
| README.md | ✓ |
| CHANGELOG.md | ✓ |

---

## immich

| Type | Detail |
|------|--------|
| manifest | `.claude-plugin/plugin.json` |
| manifest | `.codex-plugin/plugin.json` |
| skill | `skills/immich/SKILL.md` |
| README.md | ✓ |
| CHANGELOG.md | ✓ |

---

## labby

| Type | Detail |
|------|--------|
| manifest | `.claude-plugin/plugin.json` |
| hook | `hooks/hooks.json` |
| skill | `skills/using-labby/SKILL.md` |
| .mcp.json | `lab` -> `${user_config.server_url}/mcp` |
| README.md | ✓ |
| CHANGELOG.md | ✓ |

---

## linkding

| Type | Detail |
|------|--------|
| manifest | `.claude-plugin/plugin.json` |
| manifest | `.codex-plugin/plugin.json` |
| skill | `skills/linkding/SKILL.md` |
| README.md | ✓ |
| CHANGELOG.md | ✓ |

---

## loggifly

| Type | Detail |
|------|--------|
| manifest | `.claude-plugin/plugin.json` |
| manifest | `.codex-plugin/plugin.json` |
| skill | `skills/loggifly/SKILL.md` |
| README.md | ✓ |
| CHANGELOG.md | ✓ |

---

## memos

| Type | Detail |
|------|--------|
| manifest | `.claude-plugin/plugin.json` |
| manifest | `.codex-plugin/plugin.json` |
| skill | `skills/memos/SKILL.md` |
| README.md | ✓ |
| CHANGELOG.md | ✓ |

---

## navidrome

| Type | Detail |
|------|--------|
| manifest | `.claude-plugin/plugin.json` |
| manifest | `.codex-plugin/plugin.json` |
| hook | `hooks/hooks.json` |
| script | `scripts/setup.sh` |
| skill | `skills/navidrome/SKILL.md` |
| README.md | ✓ |
| CHANGELOG.md | ✓ |

---

## neo4j

| Type | Detail |
|------|--------|
| manifest | `.claude-plugin/plugin.json` |
| manifest | `.codex-plugin/plugin.json` |
| skill | `skills/neo4j/SKILL.md` |
| README.md | ✓ |
| CHANGELOG.md | ✓ |

---

## notebooklm

| Type | Detail |
|------|--------|
| manifest | `.claude-plugin/plugin.json` |
| manifest | `.codex-plugin/plugin.json` |
| skill | `skills/notebooklm/SKILL.md` |
| README.md | ✓ |
| CHANGELOG.md | ✓ |

---

## plexus

| Type | Detail |
|------|--------|
| manifest | `.claude-plugin/plugin.json` |
| manifest | `.codex-plugin/plugin.json` |
| command | `commands/remote-context.md` |
| hook | `hooks/hooks.json` |
| script | `scripts/remote-context.py` |
| skill | `skills/bootstrap-plexus/SKILL.md` |
| skill | `skills/operating-remote/SKILL.md` |
| README.md | ✓ |
| CHANGELOG.md | ✓ |

---

## qdrant

| Type | Detail |
|------|--------|
| manifest | `.claude-plugin/plugin.json` |
| manifest | `.codex-plugin/plugin.json` |
| skill | `skills/qdrant/SKILL.md` |
| README.md | ✓ |
| CHANGELOG.md | ✓ |

---

## radicale

| Type | Detail |
|------|--------|
| manifest | `.claude-plugin/plugin.json` |
| manifest | `.codex-plugin/plugin.json` |
| skill | `skills/radicale/SKILL.md` |
| README.md | ✓ |
| CHANGELOG.md | ✓ |

---

## scripts

No registered components found.

---

## scrutiny

| Type | Detail |
|------|--------|
| manifest | `.claude-plugin/plugin.json` |
| manifest | `.codex-plugin/plugin.json` |
| skill | `skills/scrutiny/SKILL.md` |
| README.md | ✓ |
| CHANGELOG.md | ✓ |

---

## swag

| Type | Detail |
|------|--------|
| manifest | `.claude-plugin/plugin.json` |
| manifest | `.codex-plugin/plugin.json` |
| hook | `hooks/hooks.json` |
| skill | `skills/swag/SKILL.md` |
| .mcp.json | `swag-mcp` -> `uv run --project ${CLAUDE_PLUGIN_ROOT} python -m swag_mcp` |
| .mcp.json | `swag-mcp-remote` -> `mcp-remote ${SWAG_MCP_URL}` |
| README.md | ✓ |
| CHANGELOG.md | ✓ |

---

## tei

| Type | Detail |
|------|--------|
| manifest | `.claude-plugin/plugin.json` |
| manifest | `.codex-plugin/plugin.json` |
| skill | `skills/tei/SKILL.md` |
| README.md | ✓ |
| CHANGELOG.md | ✓ |

---

## testing

| Type | Detail |
|------|--------|
| manifest | `.claude-plugin/plugin.json` |
| manifest | `.codex-plugin/plugin.json` |
| skill | `skills/android-app-testing/SKILL.md` |
| skill | `skills/claude-in-mobile/SKILL.md` |
| skill | `skills/desktop-app-testing/SKILL.md` |
| skill | `skills/mcpjam-ui-testing/SKILL.md` |
| skill | `skills/mcporter/SKILL.md` |
| skill | `skills/web-app-testing/SKILL.md` |
| README.md | ✓ |
| CHANGELOG.md | ✓ |

---

## uptime-kuma

| Type | Detail |
|------|--------|
| manifest | `.claude-plugin/plugin.json` |
| manifest | `.codex-plugin/plugin.json` |
| hook | `hooks/hooks.json` |
| script | `scripts/setup.sh` |
| skill | `skills/uptime-kuma/SKILL.md` |
| README.md | ✓ |
| CHANGELOG.md | ✓ |

---

## vibin

| Type | Detail |
|------|--------|
| manifest | `.claude-plugin/plugin.json` |
| manifest | `.codex-plugin/plugin.json` |
| command | `commands/scaffold-claude-plugin.md` |
| monitor | `monitors/monitors.json` |
| skill | `skills/agent-os/SKILL.md` |
| skill | `skills/aurora-design-system/SKILL.md` |
| skill | `skills/check-skill-clis/SKILL.md` |
| skill | `skills/chrome/SKILL.md` |
| skill | `skills/claude-android-ninja/SKILL.md` |
| skill | `skills/clipboard/SKILL.md` |
| skill | `skills/create-swag-config/SKILL.md` |
| skill | `skills/desktop-app-testing/SKILL.md` |
| skill | `skills/fastmcp-client-cli/SKILL.md` |
| skill | `skills/gh-fix-ci/SKILL.md` |
| skill | `skills/gh-pr/SKILL.md` |
| skill | `skills/hand-off/SKILL.md` |
| skill | `skills/homelab-map/SKILL.md` |
| skill | `skills/jetpack-compose-expert/SKILL.md` |
| skill | `skills/mcp-gateway-tools/SKILL.md` |
| skill | `skills/nircmd/SKILL.md` |
| skill | `skills/paperless-ngx/SKILL.md` |
| skill | `skills/quick-push/SKILL.md` |
| skill | `skills/rclone/SKILL.md` |
| skill | `skills/refresh-docs/SKILL.md` |
| skill | `skills/save-to-md/SKILL.md` |
| skill | `skills/screenshots/SKILL.md` |
| skill | `skills/summarize/SKILL.md` |
| skill | `skills/sysinternals/SKILL.md` |
| skill | `skills/using-rmcp/SKILL.md` |
| skill | `skills/validate-skill/SKILL.md` |
| skill | `skills/work-it/SKILL.md` |
| skill | `skills/yt-dlp/SKILL.md` |
| .mcp.json | *(empty — no servers registered)* |
| README.md | ✓ |
| CHANGELOG.md | ✓ |
