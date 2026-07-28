# Current Labby Surfaces

Do not maintain a second hand-written action inventory here. The code-owned
sources are:

- `docs/generated/service-catalog.md`
- `docs/generated/action-catalog.md`
- `docs/generated/mcp-help.md`
- `docs/generated/cli-help.md`

## Services

| Service | Exposure | Surfaces |
| --- | --- | --- |
| `gateway` | feature-gated | CLI, MCP, API, web |
| `fs` | feature-gated | MCP, API, web |
| `doctor` | always on | CLI, MCP, API |
| `server_logs` | always on | CLI, MCP, API |
| `setup` | always on | CLI, MCP, API, web |
| `snippets` | always on | CLI, MCP, API |
| `lab_admin` | runtime-conditional | CLI, MCP |

For an action-based service, call `help` or `schema` before sending complex
parameters. Destructive metadata and required confirmation come from the shared
action catalog.

## Gateway Resources

- `lab://gateway/servers` lists registered upstreams and capability counts.
- `lab://gateway/<name>/schema` returns the exposed schema for one upstream.

Code Mode also provides catalog search and description for hidden upstream tools.

## Product Boundary

ACP, the ACP Agent Registry, the in-product MCP Registry browser/client,
Marketplace, Fleet/device runtime, Deploy-product, and Stash are not current
services. Do not infer availability from historical docs or old release notes.
