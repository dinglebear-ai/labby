# labby-runtime — Surface-Neutral Runtime Contracts

This crate owns reusable runtime DTOs and helpers shared across extracted Labby crates without depending on product transports.

## Boundaries

Allowed concerns include stable agent-error vocabulary, caller auth context, gateway configuration DTOs, catalog notifications, path/redaction helpers, and Agent Skills parsing/wire contracts.

Do not depend on `axum`, `clap`, `rmcp`, Javy/Wasmtime, or Labby's product service registry. Gateway-only process/SSRF/dispatch behavior belongs in `labby-gateway`; Code Mode execution belongs in `labby-codemode`.

Keep config structures serialization-stable and avoid ambient product configuration reads unless the helper is explicitly defined as a generic environment/path primitive.
