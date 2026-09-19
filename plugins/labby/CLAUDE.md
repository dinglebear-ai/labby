# Labby Plugin Package Instructions

`plugins/labby` is the checked-in first-party plugin and Agent Skill package
for connecting agents to Labby and guiding a new operator through installation.

`AGENTS.md` and `GEMINI.md` in this directory are symlinks to this file.
Keep this file canonical and preserve those symlinks.

## Package boundaries

- The package does not bundle the `labby` binary.
- `skills/install-labby` is the first-class guided installation/orchestration
  surface for humans using a skill-aware agent.
- The Labby binary remains authoritative for durable setup, credentials,
  service installation, repair, gateway mutation, and validation.
- Do not reimplement binary-owned setup behavior in skill shell snippets when a
  current `labby setup`, `setup.settings.*`, gateway, or doctor action exists.
- Do not reintroduce the retired automatic Claude Code install/repair hooks.

## Skill rules

- Keep `.claude-plugin/plugin.json`, `.mcp.json`, Agent Skills, and package
  documentation aligned with the current Labby MCP/CLI surface.
- The MCP server key `lab` is intentional compatibility vocabulary. Do not
  rename it solely because the product is named Labby.
- Skills should discover current action/tool names and inspect current
  `--help`/schemas rather than hard-code catalogs that can drift.
- Installation must use an immutable release whose installer attestation and
  SHA-256 sidecar are verified before execution. Never teach raw branch
  curl-pipe-shell as the supported path.
- Secrets belong in `LABBY_HOME/.env`; non-secret preferences belong in
  `LABBY_HOME/config.toml`.
- Never commit or echo host tokens, OAuth credentials, provider API keys, or
  machine-specific secrets.
- OAuth setup must reflect the runtime contract: bearer-only, OAuth-only, or
  OAuth + static bearer are valid topologies; OAuth selects exactly one inbound
  provider, Google or Authelia.
- Codex App Server is an agent/assistant protocol, not an MCP upstream. Do not
  describe or configure it as one.
- The checked-in `.mcp.json` statically injects the plugin bearer token and must
  be treated as a bearer convenience. OAuth-oriented Claude clients need a
  native higher-precedence MCP registration without that header plus the
  client's supported OAuth login flow.
- Reverse-proxy edits require current official upstream docs, verified backups,
  an exact proposed change, and explicit operator approval before mutation.
- Installation is not complete until doctor/readiness plus a live MCP smoke
  succeeds.

## Validation

When changing this package or the setup surfaces it documents:

1. run focused Rust tests for the affected setup behavior;
2. regenerate generated CLI/action docs when command grammar changes;
3. run the repository docs checks;
4. inspect the package for stale private-host defaults and secrets;
5. preserve the `AGENTS.md` and `GEMINI.md` symlinks.

See `docs/PLUGINS.md`,
`docs/adr/0001-install-labby-first-class-install-orchestrator.md`, and the
generated CLI/action catalogs for the current contract.
