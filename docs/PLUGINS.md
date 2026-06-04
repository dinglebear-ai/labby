# Lab Plugins

`labby marketplace generate --out <dir>` builds a Claude Code plugin marketplace tree from the compiled service registry and each service `PluginMeta`.

The generated tree contains:

- `lab-core/`: the copied release binary at `bin/labby`, setup commands, and an install-binary skill.
- `lab-<service>/`: one config-only service plugin per service with required env vars.
- `.claude-plugin/marketplace.json` and/or `.agents/plugins/marketplace.json`: indexes for generated plugin marketplaces.

Service plugins do not depend on `PATH`. Their `.mcp.json` points at:

```json
"${HOME}/.claude/plugins/lab-core/bin/labby"
```

The core plugin provides:

- `/setup-core`: opens `labby setup --mode plugin`.
- `/setup-core-advanced`: opens `labby setup --mode full`.

Plugin manifests intentionally omit `version`; marketplace release identity is Git-SHA based unless an individual plugin explicitly documents a different manifest-level version contract.

Setup plugin lifecycle actions live in the `setup` dispatch service:

- `setup.installed_plugins`
- `setup.install_plugin`
- `setup.uninstall_plugin`
- `setup.services_status`

`install_plugin` and `uninstall_plugin` validate the registered service slug, derive `lab-<service>@lab`, check the org against `LAB_PLUGIN_ALLOWLIST`, and call the configured Claude Code CLI. Set `LAB_CLAUDE_BIN` when the binary is not named `claude`.

`labby help` and `lab://catalog` are env-aware by default: services with missing required env vars are hidden. Use `LAB_SHOW_ALL=1` or `labby help --all` to show the full compiled catalog.
