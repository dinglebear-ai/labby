# Labby rmcp compatibility patch

This directory vendors `rmcp` 3.1.0 from
`dinglebear-ai/rust-sdk@c9c3e518ce94b8d715519657c88195515978bda4`.
That immutable fork commit carries Labby's protocol-generic custom-result
decoder fix. Relative to that base, this vendored crate also carries the
following compatibility deltas, all pinned and checked by
`conformance/vendor-rmcp-provenance.json`:

- an optional `Tool.security_schemes` field serialized as top-level
  `securitySchemes`, including macro initialization and wire-model tests;
- exact authorization-server issuer comparison for OAuth discovery; and
- authenticated bearer credentials in the `Authorization` header, never a
  query parameter;
- issuer-bound authorization state and stored credentials, including safe
  rejection and recovery when an issuer changes;
- fixed, non-reflecting authorization callback errors and validation before a
  code can reach a token endpoint;
- least-privilege scope selection from explicit requests and resource
  challenges without expansion to the server's full scope catalog;
- protected-resource propagation through authorization, code exchange, and
  refresh requests;
- deterministic pre-registration, CIMD, DCR, and user-supplied registration
  precedence, including typed DCR application types, authorization/refresh
  grant declarations, and recoverable registration failures; and
- explicit expired-token and refresh behavior.
- typed custom-request response decoding with raw response preservation across
  supported transports, ported from `jmagar/rust-sdk@e3bc6c71f5ee6d708fa79f860280a96788ebdf27`.

The field uses `Option<Vec<serde_json::Value>>`: omission preserves the standard
MCP 3.1 wire model, while values preserve OpenAI's current `noauth` / `oauth2`
objects and remain forward-compatible with future extension shapes.

The provenance manifest is the complete machine-checked changed-file and patch
inventory; this document summarizes the behavioral obligations. The standalone
`Cargo.toml` adaptation and deterministic formatting-only test change are
packaging deltas, not protocol behavior. Remove this vendored crate only after
every manifest obligation is available in an official `rmcp` release and
Labby's MCP/OpenAI conformance matrices pass against that release.
