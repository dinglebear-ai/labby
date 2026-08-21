# labby-gateway Instructions

`labby-gateway` is the surface-neutral upstream MCP gateway runtime. It owns upstream transports, bounded discovery, routing, relays, Code Mode host integration, capability health, annotations, OAuth-aware connection state, and gateway-level error semantics.

## Boundary Rules

- Do not depend on `clap`, the product MCP server, the web UI, or product-specific HTTP handlers.
- Keep shared gateway behavior here and adapt it from `crates/labby` surface code.
- Upstream implementations live under `src/upstream/`; gateway orchestration lives under `src/gateway/`.
- Read the nested `src/upstream/CLAUDE.md` and `src/gateway/CLAUDE.md` before editing those areas.
- Never use unbounded rmcp `Peer::list_all_*` helpers. Preserve page/item/byte/deadline bounds and repeated-cursor protection.
- Do not guess upstream tool/resource/prompt schemas. Discovery state is authoritative.

## Security

Stdio commands are trusted operator configuration but still pass through the spawn guard. Preserve SSRF, response-size, timeout, OAuth-subject, exposure-policy, admin/destructive metadata, and secret-redaction boundaries. Upstream error mapping must retain stable machine-readable kinds and recovery information.

## Testing

Use focused tests for the changed subsystem, then at minimum:

```bash
cargo test -p labby-gateway
cargo clippy -p labby-gateway --all-features --all-targets -- -D warnings
```
