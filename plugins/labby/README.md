# Labby client plugins

The canonical five skills live in `.apm/skills/`. Self-contained packages for APM's supported clients are checked in under `plugins/labby/<client>/`. Each contains a portable Agent Plugins `plugin.json`, the skill files, and any client-native project files APM can generate. The `.claude-plugin/` manifest at this directory provides the usage skills to existing Claude marketplace consumers; the separate `plugins/install-labby` package owns guided installation and HTTP MCP client configuration.

Install a client package directly with a client that supports Agent Plugins 1.0. File-based clients can copy that package's native skill directory and merge its MCP configuration into their existing settings. See each package's README before copying settings. Regenerate checked-in packages with `python3 scripts/generate-native-plugins.py` and verify them with `--check`.

The `copilot-cowork`, `copilot-app`, `grok-cloud`, and `openclaw` directories represent APM's experimental targets. Their availability depends on the client's own plugin or skill support; they are not declared as stable targets in `apm.yml`.

Labby server configuration remains owned by the Labby binary, Settings UI, and setup CLI. These packages ship no Claude Code lifecycle hooks.
