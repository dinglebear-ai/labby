---
title: "Labby Plugins"
created: "2026-07-30"
updated: "2026-09-29"
---

# Labby Plugins

The checked-in `plugins/install-labby` and `plugins/labby` trees ship **no binary**. Hosts install `labby`
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

## Checked-in plugins

The `install-labby` plugin owns the installer skill and MCP configuration; the `labby` plugin owns the usage skills. The installer plugin `.mcp.json` connects over HTTP to a
running `labby serve` (`${user_config.server_url}/mcp`), so machines that
install the plugin remotely never need a local binary at all. The plugin ships
**no Claude Code hooks**. The former SessionStart / ConfigChange setup shims,
server-environment synchronization, and per-service Claude plugin lifecycle are
retired. Plugin configuration is client-only and never mutates the Labby host.

The installer skill is the guided orchestration entry point; it delegates
durable setup and repair to the binary. The checked-in MCP connection always
injects `Authorization: Bearer ${user_config.api_token}`. Treat it as a bearer
convenience, not automatic OAuth discovery. For OAuth, register the endpoint
through the client's native MCP configuration without that static header and
use its OAuth login flow. The usage plugin contains no MCP registration.

The [implementation workflow skill](../plugins/labby/.apm/skills/implement-in-microsandbox/SKILL.md) guides explicitly authorized tasks through persistent development and retained staging; it does not add a deployment service or mutate production.

## APM package (`apm.yml`)

The repository root is also an [APM](https://microsoft.github.io/apm/) package.
`apm install -g dinglebear-ai/labby` installs the nested `plugins/labby` APM package.
Its `.apm/skills/` directory is the source of the five skills. Root `skills/`
entries are symlinks to that same source for repository discovery. The nested
package also registers the `labby` stdio MCP server through the npm launcher.
The root `apm.yml` carries a `# x-release-please-version` marker, so Release Please keeps
its version aligned with the workspace. The package ships no binary and no
hooks; host provisioning stays with `install-labby` and `labby setup`.

## Direct client packages

`plugins/labby/<client>/` contains checked-in, self-contained packages for
APM's stable targets and its experimental target names. Every directory has a
portable Agent Plugins `plugin.json` and copies of the five canonical skills;
supported clients also have APM-generated native skill and MCP paths. The
source of truth is `plugins/labby/.apm/skills/`. Run
`python3 scripts/generate-native-plugins.py --check` to detect drift.
The `native-plugins` CI job runs that generator into its `plugins/labby/`
checkout with APM 0.31.0, then fails if tracked or newly created files differ
from the commit. It also verifies the separate installer plugin's skill copy
against the canonical source. Run the generator locally and commit its output
whenever canonical skills or plugin metadata change; CI's checkout is temporary.

The portable manifest is useful for clients that load Agent Plugins directly.
Other clients load the native file paths documented in each package README.
The Claude usage plugin at `plugins/labby/` stays available for existing
marketplace consumers; `plugins/install-labby/` owns the HTTP MCP client setup.

## Marketplace distribution

Labby no longer generates or publishes an in-product plugin marketplace. The marketplace
moved to a dedicated repo, [dendrite](https://github.com/dinglebear-ai/dendrite), so it
is decoupled from this Rust workspace. Dendrite catalogs both plugin packages (via a
`git-subdir` source pointing at this repo) alongside the other Labby/Labby plugins
and third-party entries.

Install `labby` with `scripts/install.sh` (above). Plugin marketplace discovery and distribution now belong to Dendrite; Labby does not expose a `marketplace` dispatch service or marketplace web surface.

Labby does not inspect, install, or uninstall Claude Code plugins as part of the
setup service. Host configuration belongs to Labby's Settings, setup CLI, and
configuration surfaces. Capability-affecting host problems are exposed through
Doctor and the global capability-health warning instead of being hidden behind
client-plugin hooks or transport-specific guards.

`labby help` is offline and derives from the compiled Clap command graph;
`labby help --all` expands the command tree. MCP service discovery through
`lab://catalog` is a separate, environment-aware surface. Do not use CLI help
as evidence that a running gateway exposes a service.
