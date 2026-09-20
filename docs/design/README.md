# Product Design Docs

These documents describe design contracts that are implemented by, or intentionally constrain, the current Labby product.

## Accepted Target Architecture

- [labby-depot-saas-north-star.md](./labby-depot-saas-north-star.md) — hosted and self-hosted Labby/Depot product boundary, Lime first-tenant rollout, shared tenant-aware Depot target, R2 content plane, and SaaS control-plane constraints.
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

## Implementation And Alignment Records

These records document implemented UI boundaries or active acceptance work. They
are maintained because they explain constraints that are not obvious from the
component code, but they do not override the canonical contracts above.

- [browser-bridge-operator-ui.md](./browser-bridge-operator-ui.md) — implemented `/browsers` operator lifecycle and safety boundary.
- [team-authority.md](./team-authority.md) — implementation architecture for the normative [multi-user authority contract](../access-control/MULTI_USER_AUTHORITY.md).
- [discover-mock-alignment.md](./discover-mock-alignment.md) — implemented Discover alignment baseline plus an explicit ledger of the still-missing recommendation/feed contracts.
- [Unified Labby and Depot frontend](../depot-unified-frontend.md) — implemented optional-Depot frontend/runtime boundary and rollback contract.

Historical proposals and superseded implementation sketches are not indexed here as product documentation.
