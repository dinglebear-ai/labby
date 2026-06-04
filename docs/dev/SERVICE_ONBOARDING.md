# Service Onboarding

This is the end-to-end checklist for bringing a service online in `lab`.

The preferred flow is scaffold first, audit second, and all-features
verification last:

```bash
labby scaffold service <service>
labby audit onboarding <service>
cargo check --workspace --all-features
```

## Required Steps

1. Start from the upstream API spec or notes in `docs/upstream-api/`.
2. Add pure client logic and serde types under `crates/lab-apis/src/<service>/`.
3. Add the shared dispatch module under `crates/lab/src/dispatch/<service>/`.
4. Keep CLI, MCP, and HTTP adapters thin; they must call dispatch instead of
   reimplementing service behavior.
5. Register the service in feature flags, metadata, registry construction, CLI,
   MCP/API exposure, and generated docs.
6. Add or update `docs/coverage/<service>.md`.
7. Run `labby audit onboarding <service>` and fix every failed check.
8. Run the all-features build/test path before handoff.

## Source Documents

- [DISPATCH.md](./DISPATCH.md) owns the shared dispatch-layer contract.
- [ERRORS.md](./ERRORS.md) owns stable error envelopes and status mapping.
- [OBSERVABILITY.md](./OBSERVABILITY.md) owns logging, correlation, and redaction.
- [SCAFFOLD_AND_AUDIT.md](./SCAFFOLD_AND_AUDIT.md) owns scaffold/audit behavior.
