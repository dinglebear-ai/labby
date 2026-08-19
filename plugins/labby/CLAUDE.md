# Labby Plugin Package Instructions

`plugins/labby` is the checked-in Claude/plugin distribution metadata for connecting clients to an already installed Labby host. It does not ship the Labby binary and does not own host bootstrap.

## Rules

- Keep `.claude-plugin/plugin.json`, `.mcp.json`, skill metadata, and documentation aligned with the current Labby MCP surface.
- The MCP server key `lab` is intentional compatibility vocabulary; do not rename it just because the product is named Labby.
- Do not reintroduce the retired automatic Claude Code install/repair hooks. Setup and repair belong to the Labby binary.
- Skills should teach live discovery and current action names rather than hard-coding stale catalogs.
- Never commit host tokens, OAuth credentials, or machine-specific secrets.

See `docs/PLUGINS.md` and the generated action/MCP catalogs for current behavior.
