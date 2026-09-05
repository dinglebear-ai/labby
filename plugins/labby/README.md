# labby — Claude Code plugin

Skills and MCP configuration for the Labby homelab control plane.

This plugin does **not** bundle the `labby` binary and does not auto-install
or auto-repair anything. It ships:

- the `using-labby` skill,
- the `creating-snippets` skill for Labby Code Mode snippet authoring,
- an HTTP MCP server entry pointing at a running `labby serve`
  (`${user_config.server_url}/mcp` — remote machines never need a local binary),
- `userConfig` settings declared in `.claude-plugin/plugin.json`.

The plugin ships **no Claude Code hooks**. The former `hooks/hooks.json`
(SessionStart / ConfigChange shims) was removed; run `labby setup` yourself
after changing plugin settings.

## Installing labby (server host only)

```bash
version=vX.Y.Z
base="https://github.com/dinglebear-ai/labby/releases/download/$version"
curl -fSLO "$base/labby-install.sh"
curl -fSLO "$base/labby-install.sh.sha256"
gh attestation verify labby-install.sh \
  --repo dinglebear-ai/labby \
  --signer-workflow dinglebear-ai/labby/.github/workflows/release.yml \
  --source-ref "refs/tags/$version" \
  --deny-self-hosted-runners
shasum -a 256 -c labby-install.sh.sha256
LABBY_INSTALL_VERSION="$version" sh ./labby-install.sh
labby setup
```

The separately downloaded installer and checksum come from an explicit release.
`gh` verifies the installer's repository, release workflow, exact tag, and
hosted-runner provenance before the installer verifies and activates the
platform archive. Source fallback is disabled by default; opt in explicitly
with `LABBY_ALLOW_SOURCE_FALLBACK=1`. Successful installs retain owner-only
receipts and the prior verified artifact beneath
`~/.local/bin/.labby-install/` for offline rollback. Everything after install —
config, credentials, connectivity checks, repair — is owned by `labby setup`.
Configure the plugin with the URL of the Labby server you intend to trust; the
plugin never selects a shared hosted gateway for you.

The plugin exports its configured `server_url` as
`CLAUDE_PLUGIN_OPTION_SERVER_URL`. Plugin-launched Labby processes use that same
authoritative base for MCP transport, gateway management, Code Mode, and stdio
bridging, paired only with `CLAUDE_PLUGIN_OPTION_API_TOKEN`; they never inherit
an ambient `LABBY_MCP_HTTP_TOKEN` for a different authority. If the configured server fails, Labby reports the failure instead of
silently reading or executing against the invoking user's local/XDG config.

## Configuration

Plugin settings (server URL, auth mode, token, …) are declared in
`.claude-plugin/plugin.json` `userConfig`. Sync them into `~/.labby/.env` by
running `labby setup plugin-hook` manually after changing settings — this is no
longer triggered automatically by a ConfigChange hook.

The `server_url` setting is persisted as `LABBY_SERVER_URL`. Connectivity
checks prefer the invocation-scoped plugin setting, then that persisted client
target, and use Dookie's `http://localhost:40100` host proxy only when neither
is configured. Production Labby remains container-local on port 8765.

When upstream OAuth is configured, set `public_url` to the explicit public base
URL for the Labby server. Labby derives the upstream browser callback from that
value and refuses to initialize the HTTP OAuth runtime when it is missing; the
plugin does not provide a shared hosted callback.
