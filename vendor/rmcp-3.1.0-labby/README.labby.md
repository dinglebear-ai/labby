# Labby rmcp compatibility patch

This directory vendors `rmcp` 3.1.0 from
`dinglebear-ai/rust-sdk@c9c3e518ce94b8d715519657c88195515978bda4`.
That immutable fork commit carries Labby's protocol-generic custom-result decoder
fix. Labby additionally adds one optional `Tool.security_schemes` field, serialized
as top-level `securitySchemes`, plus a builder and wire-model tests.

The field uses `Option<Vec<serde_json::Value>>`: omission preserves the standard
MCP 3.1 wire model, while values preserve OpenAI's current `noauth` / `oauth2`
objects and remain forward-compatible with future extension shapes.

Remove this vendored patch after both changes are available in an official `rmcp`
release.
