# Vendored dependency patches

## rmcp 3.1.0

`vendor/rmcp` is the Cargo-normalized source package for the official crates.io `rmcp` 3.1.0 release, with one temporary decoder correction in `src/model.rs`.

Provenance:

- Upstream package: `rmcp` 3.1.0, Apache-2.0
- Upstream repository: `https://github.com/modelcontextprotocol/rust-sdk`
- Patch development fork: `dinglebear-ai/rust-sdk`
- Patch commit: `c9c3e518ce94b8d715519657c88195515978bda4`
- Package construction: `cargo package -p rmcp --allow-dirty --no-verify`, then extracted locally

The patch changes `CallToolResult` deserialization so protocol-generic `resultType` and `_meta` fields are not sufficient to classify an arbitrary custom extension response as a tool result. At least one tool-specific field (`content`, `structuredContent`, or `isError`) must be present. Two regression tests preserve both sides of the boundary: a Depot-shaped Skills result remains `CustomResult`, while a real tool result containing `_meta` remains `CallToolResult`.

The vendored tree is intentionally used instead of allowing Git dependencies in `deny.toml`; Labby keeps its `unknown-git = "deny"` / empty `allow-git` supply-chain policy. Remove this vendor override as soon as an upstream rmcp release contains the equivalent decoder fix, then restore the normal crates.io dependency and regenerate `Cargo.lock`.
