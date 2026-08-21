# crates/labby/src — Product Surfaces And Dispatch

This directory is the Labby product layer. Reusable runtime behavior should live in extracted crates; this tree composes those crates into product services and surfaces.

## Layer Direction

Typical flow is:

```
CLI / MCP / API / web adapter
            ↓
   product dispatch layer
            ↓
reusable labby-* runtime crates
            ↓
 upstream MCP / local host / pure SDK
```

Do not force every operation through `labby-apis`. That crate now owns only pure SDK contracts that belong there. Gateway semantics belong in `labby-gateway`; Code Mode runtime in `labby-codemode`; auth in `labby-auth`.

## Ownership

- `dispatch/`: action semantics, validation, local product services, adapter-neutral orchestration
- `cli/`: clap parsing and human/JSON presentation adapters
- `mcp/`: MCP transport/protocol adaptation, resources, apps, elicitation
- `api/`: axum routing/middleware and HTTP mapping
- `output/`: shared human CLI rendering
- `config.rs`: product config/env loading
- `registry.rs` / `catalog.rs`: shared product discovery

## Rules

- if multiple surfaces need the behavior, it belongs below those surfaces
- do not put HTTP-client/upstream pool internals in CLI/MCP/API handlers
- do not leak presentation concerns into reusable crates
- preserve typed errors until the surface boundary
- use canonical `cli` / `mcp` / `api` observability surface names
- keep current service registration aligned with generated catalogs

Read the nearest nested `CLAUDE.md` before editing a surface or dispatch tree.
