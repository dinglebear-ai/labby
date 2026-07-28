# TUI

The Ratatui plugin-manager surface is not part of the current `labby` CLI or
runtime. Older docs referred to `lab plugins`, compiled-in service metadata
tabs, and `.mcp.json` patching from a TUI; that surface is currently deferred.

Current operator surfaces are:

- CLI: `labby gateway`, `labby setup`, `labby server-logs`,
  `labby snippets`, `labby doctor`, `labby health`, `labby docs`, and host/runtime
  helpers such as `labby incus` and `labby oauth`
- MCP: `labby mcp` and hosted `/mcp`
- HTTP/Web: `labby serve`
- Labby web UI for gateway, settings, docs, snippets, usage, and design-system
  workflows

If a TUI is restored later, it should consume the generated service catalog and
feature matrix instead of hardcoding service categories or assuming removed
first-party upstream integrations.
