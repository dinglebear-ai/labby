# labby Binary Crate

`crates/labby` builds the `labby` product binary. It owns the CLI, MCP server,
HTTP API, Labby web serving, config loading, output rendering, generated docs,
and shared product dispatch.

Pure SDK/data types live in `crates/labby-apis`. HTTP/OAuth middleware lives in
`crates/labby-auth`. Windows process-tree reaping lives in
`crates/labby-winjob`.

## Build

```bash
cargo build -p labby --all-features
cargo build -p labby --no-default-features --features gateway
cargo build -p labby --no-default-features --features fs
```

## Run

```bash
labby --help
labby mcp
labby serve
labby doctor
labby gateway list
```

`labby serve` hosts the product HTTP API, streamable HTTP MCP at `/mcp`, auth
routes, and the Labby web UI when exported assets are available.

## Feature Slices

Supported standalone product slices:

- `gateway`
- `fs`

Always-on operator services are `doctor`, `server_logs`, `setup`, and
`snippets`. The `lab_admin` service is available only when explicitly enabled
at runtime.

## Dispatch

Services expose a standard action shape across MCP and HTTP:

```json
{
  "action": "mcp.list",
  "params": { "search": "github", "limit": 10 }
}
```

The registry and action catalog are generated from code-owned metadata. See the
workspace README and `docs/` for the current public contract.
