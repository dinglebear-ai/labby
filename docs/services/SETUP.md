---
title: "Setup Service"
created: "2026-08-18"
updated: "2026-08-23"
---

# Setup Service

The `setup` service owns Labby's first-run, configuration, repair, plugin-lifecycle, proxy-configuration, and host-provisioning workflows. It is always compiled and is exposed through CLI, MCP, HTTP API, and the web UI.

The generated [action catalog](../generated/action-catalog.md) is authoritative for exact action names, parameters, destructive flags, scopes, and surface availability.

## Responsibilities

- bootstrap a new Labby home and supported host runtime
- inspect setup state and service status
- stage, commit, and discard configuration drafts
- expose schema-driven settings state and mutations
- configure the direct stdio MCP proxy
- install, uninstall, inspect, and synchronize the checked-in Claude plugin integration
- repair supported setup state
- project observational access-store health into setup checks without owning access-store repair

## Safety Model

Read-only discovery actions such as `check`, `help`, `schema`, and `schema.get` do not require destructive confirmation. Mutating setup actions are classified as destructive and require `lab:admin` where the action catalog says so.

Plugin lifecycle and other local host mutations are additionally constrained by the product's local-action policy. Surface adapters must use the shared setup dispatcher rather than reimplementing setup behavior.

### Access-store projection

`setup check` and the check phase of `setup repair` include an `access_store` check derived from the same read-only health inspection as `doctor access.check`:

- `ready` passes.
- `missing` and `uninitialized` are advisory while access enforcement is disabled; the operator must use the explicit owner-bootstrap workflow before enabling enforcement.
- `insecure`, `corrupt`, `newer_schema`, `locked`, `read_only`, and `unavailable` are blocking failures.

`setup repair` never creates, migrates, bootstraps, chmods, checkpoints, or repairs `access.db` or its SQLite sidecars. Access-store recovery requires an explicit access-control workflow so setup repair cannot silently change authorization state or ownership.

Access-bootstrap proof, credential, identity, and journal files are bounded to
1 MiB each. Reads verify private ownership, file type, and hard-link count before
loading content. Windows publication applies the owner-only policy before writing
bytes and publishes the completed file without overwriting an existing artifact.
Recovery verifies the content digest plus the full file and parent-directory
identities before deleting through the verified Windows handle. Junctions,
alternate data streams, replaced files/parents, and inherited or foreign access
rules on files are refused; no pathname-delete fallback is used. New bootstrap
directories are made private. Existing parent directories may inherit rules,
but their owner and all write/delete-child/ACL-change authority must be limited
to the current user, Windows SYSTEM, or local Administrators. Unsafe existing
parents are refused without rewriting their permissions. Windows files are flushed
before atomic publication, but the platform does not provide the Unix parent
directory synchronization guarantee through the portable filesystem API.

## Main Action Families

| Family | Examples |
| --- | --- |
| Bootstrap and repair | `bootstrap`, `check`, `repair`, `finalize` |
| Draft configuration | `draft.get`, `draft.set`, `draft.commit`, `draft.discard` |
| Settings | `settings.state`, `settings.schema`, `settings.env_schema`, `settings.update`, `settings.env.update`, `settings.config.update` |
| Plugin lifecycle | `plugin.install`, `plugin.uninstall`, `plugins.installed`, `plugin_hook`, `plugin_sync`, `plugin_export` |
| Proxy | `proxy.configure` |
| Service inspection | `services.status`, `state` |

Legacy snake-case plugin action aliases remain in the action catalog for compatibility; new integrations should use the dotted canonical action names.

## CLI

`labby setup` is the supported operator entrypoint. Use `labby setup --help` and the generated [CLI help](../generated/cli-help.md) for the exact current command grammar.

## Related Docs

- [Configuration](../runtime/CONFIG.md)
- [Environment](../runtime/ENV.md)
- [Host gateway runtime](../runtime/HOST_GATEWAY.md)
- [Incus runtime](../runtime/INCUS.md)
- [Plugins](../PLUGINS.md)
- [Service model](../dev/SERVICES.md)
