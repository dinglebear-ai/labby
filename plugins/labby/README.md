# Labby plugin and Agent Skills

First-party Agent Skills and MCP configuration for the Labby control plane.

The recommended first-run path is the `install-labby` skill. Install only that
skill with the Skills CLI, then ask your agent to run it:

```bash
npx skills add https://github.com/dinglebear-ai/labby --skill install-labby
```

Then, in a skill-aware agent such as Codex:

```text
$install-labby
```

The skill is the guided operator layer. It inspects the target machine, explains
security and deployment choices, drives the verified release installer and
`labby setup`, configures the supported persistence/exposure path, verifies the
live MCP transport, and helps wire installed agents to the resulting Labby
server. The Labby binary remains the authority for durable configuration,
credentials, service installation, repair, gateway state, and validation.

## What this package ships

- `install-labby` for first-run installation and end-to-end onboarding.
- `using-labby` for day-to-day CLI, MCP, HTTP API, gateway, and operator work.
- `creating-snippets` for Code Mode snippet authoring.
- an HTTP MCP server entry pointing at `${user_config.server_url}/mcp`.
- Claude plugin `userConfig` metadata in `.claude-plugin/plugin.json`.

The package does **not** bundle the `labby` binary and ships no automatic
Claude Code install/repair hooks. The former SessionStart / ConfigChange hooks
were removed. Installation is explicit and verification-gated.

## Manual verified install

Use this when a skill-aware agent is unavailable or when you intentionally want
to perform the release bootstrap by hand:

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
```

On Linux, `sha256sum -c` is also valid.

The installer verifies and atomically activates the immutable release binary,
then enters the same `labby setup` flow used by `$install-labby`. Source
fallback is disabled unless `LABBY_ALLOW_SOURCE_FALLBACK=1` is explicitly set.
Successful installs retain owner-only receipts and the previous verified
artifact under `~/.local/bin/.labby-install/` for offline rollback.

## Setup ownership

Labby keeps security-sensitive responsibilities in the binary rather than in
skill prose:

- `.env` contains secrets and is written through the atomic, backup-backed,
  owner-only environment merge path.
- `config.toml` contains non-secret product preferences.
- `labby setup` owns first-run server/client configuration.
- `setup.settings.*` owns schema-driven settings mutation.
- `labby gateway ...` owns gateway discovery/import and Code Mode state.
- `labby doctor` is the primary configuration/runtime audit.

The install skill deliberately calls those surfaces instead of hand-writing
Labby-owned state when a first-party mutation path exists.

## Authentication

The server supports three installation topologies:

| Topology | Setup selector | Notes |
| --- | --- | --- |
| Bearer only | `--auth bearer --oauth none` | Generated static bearer credential |
| OAuth only | `--auth oauth --oauth google|authelia` | No static bearer break-glass credential |
| OAuth + bearer | `--auth both --oauth google|authelia` | OAuth plus generated static bearer |

OAuth mode selects exactly **one** inbound provider per Labby instance:
Google or Authelia. The runtime does not currently activate both providers
simultaneously.

The plugin's `auth_mode` setting maps to the lower-level runtime mode
(`bearer` or `oauth`). In OAuth + bearer deployments it remains `oauth`,
while `api_token` carries the optional static break-glass credential.

## Plugin MCP configuration

`.mcp.json` connects the plugin's compatibility server key `lab` to:

```text
${user_config.server_url}/mcp
```

The default server URL is `http://127.0.0.1:8765`, Labby's normal local
listener. Remote users must explicitly select the Labby authority they intend
to trust.

The bundled MCP definition includes a static bearer `Authorization` header and is therefore a **bearer transport convenience**, not a universal OAuth client definition. Current Claude Code does not fall back to OAuth after an explicitly configured Authorization header is rejected. `$install-labby` configures OAuth-oriented Claude clients through a higher-precedence native client registration without the static header, then uses Claude's current OAuth login flow. OAuth + bearer deployments can deliberately choose either client credential path.

The plugin exports its configured server URL as
`CLAUDE_PLUGIN_OPTION_SERVER_URL`. Plugin-launched Labby processes use that
authority with the plugin-scoped API token and do not silently fall back to an
unrelated local credential.

After changing plugin settings, use the explicit setup/plugin synchronization
surface. Nothing is auto-repaired at session start.

## Development

Package-local instructions live in `CLAUDE.md`. `AGENTS.md` and `GEMINI.md`
are symlinks to that canonical file so Codex, Claude Code, and Gemini consume
the same package rules without documentation drift.

For the installation architecture decision, see
`docs/adr/0001-install-labby-first-class-install-orchestrator.md`.
