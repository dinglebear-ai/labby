# Current Labby Surfaces

Do not maintain a second hand-written action inventory here. The code-owned
sources are:

- `docs/generated/service-catalog.md`
- `docs/generated/action-catalog.md`
- `docs/generated/mcp-help.md`
- `docs/generated/cli-help.md`

## Services

Read `docs/generated/service-catalog.md` for the complete current service list,
feature exposure, surfaces, and metadata. Do not copy that inventory into this
reference; generated discovery changes with the registered product surface.

For an action-based service, call `help` or `schema` before sending complex
parameters. Destructive metadata and required confirmation come from the shared
action catalog.

## Gateway Resources

- `lab://gateway/servers` lists registered upstreams and capability counts.
- `lab://gateway/<name>/schema` returns the exposed schema for one upstream.

Code Mode also provides catalog search and description for hidden upstream tools.

## Product Boundary

Generated discovery is authoritative. Do not infer unavailable services from
historical docs or old release notes.
