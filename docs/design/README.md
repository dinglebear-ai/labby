# Product Design Docs

These documents describe design contracts that are implemented by, or intentionally constrain, the current Labby product.

## Accepted Target Architecture

- [phabby-control-plane.md](./phabby-control-plane.md) — Phoenix/OTP control plane shared by Labby and Depot.
- [phabby-migration-ledger.md](./phabby-migration-ledger.md) — current-to-target cutover and verification gates.

## Canonical Design Contracts

- [design-system-contract.md](./design-system-contract.md) — Aurora web UI tokens, component patterns, accessibility, and layout rules.
- [component-development.md](./component-development.md) — workflow for building and reviewing Labby UI components.
- [CLI_DESIGN_SYSTEM.md](./CLI_DESIGN_SYSTEM.md) — current human-readable CLI design contract. The implementation lives in `crates/labby/src/output/` and `crates/labby/src/cli/style.rs`.
- [CLAUDE_CODE_AURORA_THEME.md](./CLAUDE_CODE_AURORA_THEME.md) — Aurora theme mapping for Claude Code.
- [SERIALIZATION.md](./SERIALIZATION.md) — serialization and output-boundary contract across product surfaces.
- [GOOGLE_CREDENTIAL_BROKER.md](./GOOGLE_CREDENTIAL_BROKER.md) — current Google credential ownership and broker design.
- [REMOTE_GATEWAY_TARGET.md](./REMOTE_GATEWAY_TARGET.md) — explicit versus opportunistic remote gateway target behavior.
- [INBOUND_IDENTITY_PROVIDER.md](./INBOUND_IDENTITY_PROVIDER.md) — accepted Google/Authelia inbound identity, migration, renewal, and provider-generation contract.

Historical proposals and superseded implementation sketches are not indexed here as product documentation.
