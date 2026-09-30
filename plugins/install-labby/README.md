# install-labby Claude Code plugin

This package ships the guided installation skill and an MCP connection definition.
Installing the plugin itself does not configure, bootstrap, repair, or mutate
the Labby server host; the skill guides the operator through those steps.

The plugin contains:

- the `install-labby` skill for guided setup and live verification,
- an HTTP MCP server entry targeting `${user_config.server_url}/mcp`,
- client-only connection settings for `server_url` and an optional bearer
  `api_token`.

There are **no Claude Code lifecycle hooks**. The old SessionStart and
ConfigChange setup shims, server-environment synchronization, per-service Claude
plugin installation, and `labby setup plugin-hook` compatibility path are
retired. Installing or reconfiguring this plugin must never mutate
`~/.labby/.env` on the machine running Labby.

## Server configuration

Configure the Labby server on the server host using Labby's own Settings UI,
configuration file, environment, or CLI. Server concerns such as OAuth,
public URLs, CORS, admin exposure, logging, and upstream credentials intentionally
are not plugin `userConfig` fields.

If a server-side setting disables or degrades a capability, Labby exposes that
through capability health and Doctor instead of requiring the client plugin to
repair server configuration.

## Client configuration

Set `server_url` to the Labby endpoint this Claude Code client should trust.
The plugin appends `/mcp`. Supply `api_token` only when that endpoint uses
bearer authentication.

The plugin does not fall back to a different Labby authority when the configured
endpoint fails. Connection failures are reported by the MCP client rather than
silently switching to local/XDG server configuration.

## Installing Labby on a server host

Install the Labby binary separately on the host that runs the control plane and
then configure that host with Labby's own setup/settings surfaces. This plugin
guides installation but does not bundle or automatically run the binary.

Install the separate `labby` plugin for day-to-day gateway, Code Mode, and snippet skills.
