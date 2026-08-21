# plugins/labby — Claude Code Plugin Package

This directory ships Labby skills, plugin metadata, user configuration, and the HTTP MCP connection definition. It does not ship the Labby binary and does not own host provisioning.

## Rules

- keep `.claude-plugin/plugin.json`, `.mcp.json`, README, skills, and generated/reference skill content aligned
- the configured `server_url` is an authoritative remote target; do not silently fall back to unrelated local Labby state
- plugin-scoped credentials must not inherit an ambient administrator token for another authority
- do not reintroduce the retired SessionStart/ConfigChange hook bootstrap system
- setup/repair belongs to the Labby binary
- keep skills agent-usable: discover exact live tools and schemas rather than teaching guessed upstream calls

Run the plugin validation/skill-drift checks after changes.
