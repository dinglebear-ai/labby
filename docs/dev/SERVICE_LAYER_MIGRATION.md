# Service Layer Migration

The original service-layer migration plan targeted first-party upstream
integrations that are no longer present in this checkout's Cargo feature table.
Treat older references to services such as Example upstream, ByteStash, and UniFi as
historical context, not current implementation guidance.

Current shared-dispatch guidance lives in:

- [DISPATCH.md](./DISPATCH.md)
- [SERVICE_ONBOARDING.md](./SERVICE_ONBOARDING.md)
- [SERVICES.md](./SERVICES.md)

The current product shape is:

- shared execution belongs under `crates/labby/src/dispatch/`
- CLI, MCP, HTTP, and web adapters stay thin over dispatch
- standalone product slices are `gateway` and `fs`
- `doctor`, `server_logs`, `setup`, and `snippets` are always-on services
- retired product features are deleted rather than preserved as aliases

If first-party upstream integrations are reintroduced, start from
`SERVICE_ONBOARDING.md`, update Cargo features intentionally, regenerate docs,
and prove both the narrow feature slice and the all-features build.
