# labby — Claude Code plugin

Skills and MCP configuration for the Lab homelab control plane.

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
curl -fsSL https://raw.githubusercontent.com/dinglebear-ai/labby/main/scripts/install.sh | sh
labby setup
```

The script downloads the latest GitHub release for this platform
(sha256-verified) into `~/.local/bin/labby`, falling back to
`cargo install --git https://github.com/dinglebear-ai/labby --bin labby --all-features`
when no release asset exists. Everything after install — config, credentials,
connectivity checks, repair — is owned by `labby setup`. Configure the plugin
with the URL of the Labby server you intend to trust; the plugin never selects
a shared hosted gateway for you.

## Configuration

Plugin settings (server URL, auth mode, token, …) are declared in
`.claude-plugin/plugin.json` `userConfig`. Sync them into `~/.labby/.env` by
running `labby setup plugin-hook` manually after changing settings — this is no
longer triggered automatically by a ConfigChange hook.

The `server_url` setting is persisted as `LABBY_SERVER_URL`. Connectivity
checks prefer the invocation-scoped plugin setting, then that persisted client
target, and use loopback only when neither is configured.

When upstream OAuth is configured, set `public_url` to the explicit public base
URL for the Labby server. Labby derives the upstream browser callback from that
value and refuses to initialize the HTTP OAuth runtime when it is missing; the
plugin does not provide a shared hosted callback.
