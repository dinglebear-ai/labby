# Microsandbox MCP upstream over SSH

Use this reference when an operator chooses Labby to reach a Microsandbox MCP
server running on another host. Microsandbox's local backend runs on that
remote execution host. This is an optional gateway integration; normal local
CLI/SDK use and a remote Microsandbox control plane do not need Labby.

Read the current [gateway SSH stdio contract](../../../../../docs/services/GATEWAY.md#stdio-gateways)
and the installed `labby server` help before configuring an upstream. This
example does not configure a live gateway or establish VM readiness.

## Upstream template (verify host and installed gateway schema)

Use the installed Labby gateway SSH stdio contract: dedicated key and verified known-hosts file,
`IdentitiesOnly`, `BatchMode`, strict host-key checking, bounded connect and
keepalive timeouts, and `-T` (no PTY). Keep Labby's spawn guard enabled. The
operating-system user running Labby must be able to read the key and execute
the command. Verify the actual process owner before choosing a key.

```toml
[[upstream]]
name = "microsandbox-<task-id>"
enabled = false
transport = "stdio"
command = "/usr/bin/ssh"
args = [
  "-i", "<labby-service-owned-key>",
  "-o", "IdentitiesOnly=yes",
  "-o", "UserKnownHostsFile=<dedicated-known-hosts>",
  "-o", "StrictHostKeyChecking=yes",
  "-o", "BatchMode=yes",
  "-o", "ConnectTimeout=10",
  "-o", "ServerAliveInterval=30",
  "-o", "ServerAliveCountMax=3",
  "-T", "-S", "none", "-o", "ControlMaster=no",
  "<remote-user>@<verified-execution-host>",
  "/usr/bin/env", "MSB_BACKEND=local",
  "MICROSANDBOX_MCP_HOST_PATHS=<dedicated-agent-workspace>",
  "MSB_PATH=<absolute-msb>",
  "MSB_LIBKRUNFW_PATH=<absolute-libkrunfw>",
  "<absolute-npx>", "-y", "microsandbox-mcp"
]
proxy_resources = false
proxy_prompts = false
proxy_skills = false
expose_tools = []
expose_resources = []
expose_prompts = []
```

The template illustrates an `npx` launch; use the task's selected package
version or an already installed executable. Verify the resolved version and
its Node engine requirement, and check that installation chatter stays
on stderr so stdout contains only MCP protocol frames. Use an absolute `npx`
path for the noninteractive SSH command. Keep stderr available for diagnostics.

Labby republishes all discovered tools, resources, and prompts when an
`expose_*` allowlist is omitted. Keep this template disabled and empty until
the task-specific route and exact tools have been reviewed. Use a protected
route or loadout and narrow `expose_tools` for the task, then inspect the
effective client-visible catalog before enabling access. The MCP includes
installation and destructive lifecycle tools; an unrestricted shared upstream
would expose them to unrelated clients. Only expose runtime and policy
resources when needed, by exact URI.

Resolve the active `msb` binary and `libkrunfw` path under the remote SSH identity, not from an
interactive shell alone. The MCP server documents `MSB_PATH` and
`MSB_LIBKRUNFW_PATH` overrides; set them to verified absolute paths in the
remote environment when default discovery fails. Confirm `runtime_check`
and an actual bounded boot before treating the upstream as operational.

The host-path allowlist is fixed when each MCP process starts. Create a
separate upstream/process per concurrent task with that task's owned directory,
key, exposure filter, and TTL. A single shared process with a broad host root
would give every caller a wider filesystem boundary and needs a distinct
access design before use.

The official MCP server defaults host-path access to its current working
directory. Set `MICROSANDBOX_MCP_HOST_PATHS` to one task-specific directory,
leave `MICROSANDBOX_MCP_HOST_PATH_POLICY=allowlist`, and keep
`MICROSANDBOX_MCP_ENABLE_DANGEROUS=0`. Inspect the live
`microsandbox://policy` resource after connection. Use `runtime_check`,
`microsandbox://runtime`, and one safe inventory tool before any create call.
Discover current MCP tool names and schemas rather than assuming the copied
upstream skill's CLI examples map one-for-one to MCP methods. With the disabled
template above, inspect runtime and policy directly on the execution host.
To read `microsandbox://policy` and `microsandbox://runtime` through Labby,
enable `proxy_resources = true` and allowlist those exact resource URIs on
the authorized task route.

## Verification order

1. On the execution host, confirm `uname`, architecture, `msb --version`, actual CLI
   help, runtime doctor, Node and MCP versions, and the intended workspace.
   Check that the actual remote Node executable meets the current MCP
   package's engine requirement.
2. From the operating-system user running the gateway, run the exact noninteractive SSH
   command with a harmless remote identity check; confirm no stdout preamble.
3. Add the task-specific upstream with bounded exposure; run
   `labby server test <task-specific-upstream>` and read back the effective
   upstream, visible tools, and MCP policy. Do not replace the existing local
   Microsandbox runner.
4. Run one bounded non-destructive sandbox smoke on the verified remote host,
   confirm host identity and cleanup, then consider warm snapshots.

The skill itself does not change Labby configuration. Record installation,
gateway changes, and deployment evidence separately for the requested task.
