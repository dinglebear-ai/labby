---
title: "Labby Plugins"
created: "2026-07-30"
updated: "2026-08-18"
---

# Labby Plugins

The checked-in `plugins/labby` tree ships **no binary**. Hosts install `labby`
explicitly and the binary owns the setup flow from there:

```bash
# Download labby-install.sh and its checksum from one explicit vX.Y.Z release,
# verify `gh attestation verify` plus the SHA-256 sidecar, then:
LABBY_INSTALL_VERSION=vX.Y.Z sh ./labby-install.sh
labby setup
```

The root `install.sh`, the canonical `scripts/install.sh`, and the web-served
copy are generated as identical, self-contained scripts. The supported workflow
downloads the installer from an explicit release, verifies its GitHub attestation
and SHA-256 sidecar before execution, and then selects the matching immutable
release containing the platform asset, requires its SHA-256 sidecar, and
installs it into `~/.local/bin/labby`. Source fallback is disabled by default
and only occurs when `LABBY_ALLOW_SOURCE_FALLBACK=1` is explicitly set; pinned
versions remain pinned during fallback. Successful installs retain
content-addressed artifacts and an owner-only receipt under the install
directory's `.labby-install/` folder. `LABBY_INSTALL_ROLLBACK=1` restores the
prior verified executable offline without changing durable Labby state. The
installer journals the pre-install binary and receipts before activation; its
next invocation restores an interrupted activation before attempting new work.
An unrestorable journal is retained and reported rather than discarded. The
installer's only job is bootstrap; everything after first contact (config,
credentials, connectivity, repair) is owned by `labby setup`.

## Checked-in plugin (`plugins/labby`)

Skills and MCP configuration only. Its `.mcp.json` connects over HTTP to a
running `labby serve` (`${user_config.server_url}/mcp`), so machines that
install the plugin remotely never need a local binary at all. The plugin ships
**no Claude Code hooks** — the former `hooks/hooks.json` (SessionStart /
ConfigChange shims) was removed. Run `labby setup plugin-hook` manually to sync
settings, or `--no-repair` for a read-only audit. Nothing is auto-installed or
auto-repaired at session start.

## Marketplace distribution

Labby no longer generates or publishes an in-product plugin marketplace. The marketplace
moved to a dedicated repo, [dendrite](https://github.com/dinglebear-ai/dendrite), so it
is decoupled from this Rust workspace. Dendrite catalogs `plugins/labby` (via a
`git-subdir` source pointing at this repo) alongside the other Labby/Labby plugins
and third-party entries.

Install `labby` with `scripts/install.sh` (above). Plugin marketplace discovery and distribution now belong to Dendrite; Labby does not expose a `marketplace` dispatch service or marketplace web surface.

Setup plugin lifecycle actions live in the `setup` dispatch service. The
canonical names follow the dotted `<resource>.<verb>` convention; the legacy
snake_case names remain as deprecated aliases:

| Canonical | Deprecated alias |
|-----------|------------------|
| `setup.plugins.installed` | `setup.installed_plugins` |
| `setup.plugin.install` | `setup.install_plugin` |
| `setup.plugin.uninstall` | `setup.uninstall_plugin` |
| `setup.services.status` | `setup.services_status` |

These four actions are restricted to loopback-only HTTP; both the canonical and
the alias forms are gated identically.

`plugin.install` and `plugin.uninstall` validate the registered service slug, derive `lab-<service>@<org>`, require that org to match the compile-time `LABBY_PLUGIN_ORG` value (default `lab`), and call the configured Claude Code CLI. Set runtime `LABBY_CLAUDE_BIN` when the binary is not named `claude`.

`labby help` and `lab://catalog` are env-aware by default: services with missing required env vars are hidden. Use `LABBY_SHOW_ALL=1` or `labby help --all` to show the full compiled catalog.
