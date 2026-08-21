# labby-runtime Instructions

`labby-runtime` contains reusable, surface-neutral runtime contracts and helpers shared by the Labby product and extracted crates.

## Rules

- Keep this crate independent of CLI, product MCP handlers, Axum route composition, and frontend code.
- Put stable DTOs/config/runtime contracts here only when more than one product boundary needs them.
- Preserve Serde field names and optionality unless the consuming surface migration is coordinated in the same change.
- Do not read ambient environment variables or files from shared data types; pass resolved configuration in explicitly.
- Prefer typed errors and structured state over strings that force downstream reparsing.
- Agent Skills support here is runtime vocabulary/behavior, not product presentation.
- Artifact domain, validation, local persistence, and lifecycle semantics live here as the shared implementation. CLI/API/MCP surfaces must remain thin projections over this layer. Skills are an Artifact family and must keep using the existing Skills validation/compatibility path rather than growing a parallel implementation.
- Keep the frozen `dinglebear.artifact-interchange/v1` wire contract byte-compatible with the canonical Depot fixture; Labby-only storage/provider/deployment state stays outside that shared envelope.

## Verification

```bash
cargo test -p labby-runtime
cargo clippy -p labby-runtime --all-features --all-targets -- -D warnings
```
