# Incus deployment and agent-sandbox reference

Labby treats Incus as the recommended self-hosted gateway boundary when the host supports it.

Current supported release shape:
- Linux x86_64 host
- Ubuntu 26.04 system container
- unprivileged container
- nesting disabled
- TUN passthrough when Tailscale is enabled
- systemd-managed Labby service

The release image includes a bounded runtime/toolchain floor including Node/npm/npx, uv/Python, Rust, Go, Claude Code, Codex, Gemini CLI, Tailscale, ffmpeg, and Android tooling.

Labby does not silently install or initialize Incus on the host. Inspect:
- incus version
- incus admin init state
- storage pool availability
- network bridge/NAT
- host architecture

Use labby setup / labby incus setup as the supported bootstrap, not a hand-rolled container recipe.

After bootstrap verify:
- container is running
- boot.autostart=true
- labby.service is enabled and active
- loopback /ready succeeds inside the container
- expected published host address/port works
- .env and config.toml ownership/modes are correct
- TUN works if Tailscale is requested

## Agent sandbox option

Ask whether the operator wants the container to double as an isolated agent workspace.

The benefit is that agent CLIs and stdio MCP subprocesses can operate inside the container rather than directly on the host.

Do not oversell the boundary. An unprivileged container is still a shared-kernel isolation mechanism, and permissions/capabilities exposed to the container still matter.

### Codex

Codex App Server is supported by Labby's Phoenix surface and uses its own JSONL/JSON-RPC-like protocol. It is not MCP.

For a Codex-powered sandbox:
1. enter the container as the labby user
2. verify codex version
3. complete codex login using the user's chosen authentication mechanism
4. verify codex app-server starts
5. configure Phoenix's provider as codex_app_server through Labby's supported settings path when a non-default change is needed
6. use a container-local workspace root
7. smoke Phoenix through Labby

Never expose Codex App Server remotely without reviewing its current transport authentication guidance.

### Claude Code

The image includes Claude Code and supports interactive claude login.

Before attempting to expose Claude as a harness/upstream:
1. inspect claude --help and claude mcp --help
2. search current official Claude Code documentation
3. determine whether the desired direction is:
   - Claude consumes Labby as an MCP server
   - Labby invokes Claude as an enrichment/agent CLI
   - another supported harness integration
4. use the actual supported interface

Do not invent a "Claude MCP server" command merely to parallel Codex.

### Devcontainers

Labby also has a separate dev_containers capability for project environments. Do not confuse:
- the long-lived Incus gateway container
- a project devcontainer managed by Labby
- an agent harness process inside the gateway

At handoff, offer to create a devcontainer after the base Labby deployment is verified.
