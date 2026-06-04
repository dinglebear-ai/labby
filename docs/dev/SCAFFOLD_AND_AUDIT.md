# Scaffold And Audit

`labby scaffold service` and `labby audit onboarding` are the preferred guardrails
for service onboarding.

Use scaffold to create the expected module skeleton, then use audit to verify
that the service is wired into every required surface and registry.

```bash
labby scaffold service <service>
labby audit onboarding <service>
```

## Scaffold Contract

The scaffolded shape must follow the repo's module and layer rules:

- `lab-apis` owns upstream clients, serde types, and service errors.
- `crates/lab/src/dispatch/<service>/` owns action catalog, params, client
  resolution, and shared execution.
- CLI, MCP, and HTTP adapters stay thin and call dispatch.
- No `mod.rs` files are introduced.

## Audit Contract

The onboarding audit should catch missing or drifted wiring across:

- Cargo features and `lab-apis` passthrough features
- `PluginMeta` and environment metadata
- dispatch action catalog and schema
- CLI registration
- MCP/API registration
- generated docs and service coverage docs

Run the audit before all-features verification. A service is not online until
the scaffold/audit checks and the normal all-features build path pass.
