# Generated Docs

Every file in this directory, including this README, is code-owned. Do not edit generated Markdown or JSON by hand.

From the repository root, regenerate the all-feature documentation projection and run the repository documentation gates:

```bash
just docs-generate
just docs-check
```

The `just docs-check` recipe in [Justfile](../../Justfile) checks generated freshness, Markdown links and heading fragments, product-documentation rules, instruction topology, Depot control-plane contracts, and their regression fixtures.

The `labby docs check` subcommand is narrower: it compares freshly rendered content with this artifact manifest and validates generation invariants and built-in snippets. It does not replace the repository recipe and does not test live service health, deployment configuration, or a caller's actual exposure.

These catalogs describe the all-feature documentation projection, not a live server inventory. Runtime feature, platform, configuration, and authorization gates still apply.

## Source ownership

The [artifact manifest](../../crates/labby/src/docs/artifacts.rs) owns this complete inventory, including the index itself. [Shared renderers](../../crates/labby/src/docs/render.rs) own formatting. Follow the entrypoints below to their authoritative action metadata, Clap definitions, route registry, configuration descriptors, or Cargo manifests; fix those sources, then regenerate. See [Documentation Maintenance](../dev/DOCUMENTATION.md) for authored, synchronized, and historical boundaries.

| Artifact | Source entrypoint |
| --- | --- |
| [README.md](README.md) | [`crates/labby/src/docs/render.rs`](../../crates/labby/src/docs/render.rs) |
| [service-catalog.md](service-catalog.md) | [`crates/labby/src/docs/projection.rs`](../../crates/labby/src/docs/projection.rs) |
| [service-catalog.json](service-catalog.json) | [`crates/labby/src/docs/projection.rs`](../../crates/labby/src/docs/projection.rs) |
| [env-reference.md](env-reference.md) | [`crates/labby/src/docs/projection.rs`](../../crates/labby/src/docs/projection.rs) |
| [env-reference.json](env-reference.json) | [`crates/labby/src/docs/projection.rs`](../../crates/labby/src/docs/projection.rs) |
| [proxy-config-reference.md](proxy-config-reference.md) | [`crates/labby/src/docs/projection.rs`](../../crates/labby/src/docs/projection.rs) |
| [proxy-config-reference.json](proxy-config-reference.json) | [`crates/labby/src/docs/projection.rs`](../../crates/labby/src/docs/projection.rs) |
| [action-catalog.md](action-catalog.md) | [`crates/labby/src/docs/action_catalog.rs`](../../crates/labby/src/docs/action_catalog.rs) |
| [action-catalog.json](action-catalog.json) | [`crates/labby/src/docs/action_catalog.rs`](../../crates/labby/src/docs/action_catalog.rs) |
| [cli-help.md](cli-help.md) | [`crates/labby/src/cli/help.rs`](../../crates/labby/src/cli/help.rs) |
| [cli-help.json](cli-help.json) | [`crates/labby/src/cli/help.rs`](../../crates/labby/src/cli/help.rs) |
| [cli-migration.md](cli-migration.md) | [`crates/labby/src/cli/migration.rs`](../../crates/labby/src/cli/migration.rs) |
| [mcp-help.md](mcp-help.md) | [`crates/labby/src/catalog.rs`](../../crates/labby/src/catalog.rs) |
| [mcp-help.json](mcp-help.json) | [`crates/labby/src/catalog.rs`](../../crates/labby/src/catalog.rs) |
| [api-routes.md](api-routes.md) | [`crates/labby/src/docs/routes.rs`](../../crates/labby/src/docs/routes.rs) |
| [api-routes.json](api-routes.json) | [`crates/labby/src/docs/routes.rs`](../../crates/labby/src/docs/routes.rs) |
| [openapi.json](openapi.json) | [`crates/labby/src/api/openapi.rs`](../../crates/labby/src/api/openapi.rs) |
| [feature-matrix.md](feature-matrix.md) | [`crates/labby/src/docs/projection.rs`](../../crates/labby/src/docs/projection.rs) |
| [feature-matrix.json](feature-matrix.json) | [`crates/labby/src/docs/projection.rs`](../../crates/labby/src/docs/projection.rs) |
