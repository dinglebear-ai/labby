# Changelog — labby plugin

## Unreleased

- **Removed the bundled `labby` binary** (previously shipped via Git LFS at
  `bin/labby`). The plugin is now skills + MCP config only. Install the binary
  explicitly: `curl -fsSL https://raw.githubusercontent.com/jmagar/lab/main/scripts/install.sh | sh`,
  then run `labby setup`.
- Hooks are now advisory and resolve `labby` from `PATH`: SessionStart audits
  setup with `--no-repair` (no auto-repair at session start) and prints an
  install pointer when labby is missing; ConfigChange still syncs changed
  plugin settings.
