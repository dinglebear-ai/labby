# Labby for windsurf

This directory is a self-contained Labby Agent Plugin. Install this directory with a client that supports Agent Plugins 1.0. It includes the four Labby skills and, where supported, a stdio MCP connection through `npx -y @dinglebear/labby mcp`.

The client-native files in hidden directories are provided for clients that load skills and MCP settings directly. Copy the skill folders to that client's documented skills location and merge MCP settings with existing settings; do not overwrite existing config.

Generated from `plugins/labby/.apm/skills` by `python3 scripts/generate-native-plugins.py`.
