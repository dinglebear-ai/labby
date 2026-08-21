# labby-primitives Instructions

`labby-primitives` is a dependency-leaf crate for stable vocabulary shared across Labby crates. It owns action/parameter metadata, MCP constants, plugin UI metadata, and low-level security primitives such as SSRF validation.

## Rules

- Keep internal workspace dependencies at zero. Higher-level crates may depend on primitives; primitives must not depend back on them.
- Treat public enum variants, field names, action metadata, URI/scope constants, and serialization shapes as contracts.
- Keep protocol names such as `lab://`, `lab:read`, `lab`, and `lab:admin`; these intentionally survived the Lab to Labby product rename.
- `requires_admin` and `destructive` are different policy axes. Do not infer one from the other.
- Security helpers must fail closed and return enough structure for callers to explain recovery safely.

## Verification

```bash
cargo test -p labby-primitives
cargo clippy -p labby-primitives --all-targets -- -D warnings
```
