---
title: "Labby Plugins"
created: "2026-07-30"
updated: "2026-09-18"
---

# Labby Plugins

The checked-in `plugins/labby` tree ships **no binary**. Its first-class guided installation entry point is the `install-labby` Agent Skill:

```bash
npx skills add https://github.com/dinglebear-ai/labby --skill install-labby
```

A skill-aware agent can then invoke `$install-labby`. The skill owns the operator conversation, environment inspection, third-party documentation lookup, and verification sequence. The verified release installer and the Labby binary continue to own binary activation, durable configuration, credentials, persistence, repair, gateway mutation, and health checks.

For a manual installation, download `labby-install.sh` and its checksum from one explicit `vX.Y.Z` release, verify `gh attestation verify` plus the SHA-256 sidecar, then run:

```bash
LABBY_INSTALL_VERSION=vX.Y.Z sh ./labby-install.sh
```

The installer enters `labby setup` after activation unless setup was explicitly disabled.

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
An unrestorable journal is retained and reported rather than discarded. The installer remains a deterministic binary bootstrap boundary. Everything after first contact that mutates Labby-owned state is owned by typed Labby setup/config/gateway surfaces. The `install-labby` skill orchestrates those surfaces rather than duplicating them.

## Checked-in plugin (`plugins/labby`)

Skills and MCP configuration only. Its `.mcp.json` connects over HTTP to a running `labby serve` (`${user_config.server_url}/mcp`), so machines that install the plugin remotely never need a local binary at all. The public default is Labby's ordinary loopback endpoint, `http://127.0.0.1:8765`; remote clients must explicitly select the authority they trust.

The checked-in MCP entry always emits a static bearer Authorization header. It is intentionally a bearer convenience rather than a conditional OAuth definition. Claude Code does not fall back to OAuth when an explicitly configured Authorization header is rejected, so `$install-labby` registers OAuth-oriented Claude clients through the client's higher-precedence native MCP scope without that static header, then uses the supported OAuth login flow. OAuth + bearer deployments can choose either client credential path.

The package includes `install-labby` for guided first-run onboarding, `using-labby` for established deployments, and `creating-snippets` for Code Mode authoring. `AGENTS.md` and `GEMINI.md` are symlinks to the package's canonical `CLAUDE.md` so the agent instructions cannot drift independently.

The plugin ships **no Claude Code hooks**. The former `hooks/hooks.json` (SessionStart / ConfigChange shims) was removed. Run the explicit Labby setup/plugin synchronization surface when settings change. Nothing is auto-installed or auto-repaired at session start.

## Marketplace distribution

Labby no longer generates or publishes an in-product plugin marketplace. The marketplace
moved to a dedicated repo, [dendrite](https://github.com/dinglebear-ai/dendrite), so it
is decoupled from this Rust workspace. Dendrite catalogs `plugins/labby` (via a
`git-subdir` source pointing at this repo) alongside the other Labby/Labby plugins
and third-party entries.

Use `$install-labby` for the guided first-run path or the verified `scripts/install.sh` release flow above for manual bootstrap. Plugin marketplace discovery and distribution belong to Dendrite; Labby does not expose a `marketplace` dispatch service or marketplace web surface.

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
