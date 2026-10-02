# Labby Product Documentation

This directory is the canonical documentation entrypoint for the current Labby product.

The current Rust/TypeScript implementation owns product behavior. The catalogs under [generated/](./generated/README.md) describe the all-feature documentation projection, not a live deployment: platform, startup configuration, route exposure, and caller authorization still determine which operations are available. Product prose should explain these boundaries rather than preserve old product shapes.

Historical material under `docs/archive/`, plans, and dated task records is not current product authority. Completed implementation packets should identify their historical status and point to the current contracts. `docs/sessions/` and `docs/superpowers/` are protected history, excluded from a general audit, including link audits. `docs/references/` is an untracked research cache. See [Documentation Maintenance](./dev/DOCUMENTATION.md#audit-the-tree-before-editing) for classification and update rules.

## Start Here

- [Upstream runtime maintenance notes](./dev/UPSTREAM_INTERNALS.md) — on-demand implementation rationale; the scoped AGENTS.md remains the concise contributor entrypoint.
- [Contributor instructions](../AGENTS.md) — canonical root rules, with CLAUDE.md and GEMINI.md compatibility symlinks.
- [Development workflow](./dev/DEVELOPMENT.md) — checkout safety, prerequisites, owning layers, validation, and publication.
- [Documentation maintenance](./dev/DOCUMENTATION.md) — source ownership, instruction topology, protected history, regeneration, and regression gates.
- [Architecture](./ARCH.md) — workspace boundaries, runtime flow, and product surfaces.
- [Architecture decisions](./adr/README.md) — accepted and proposed durable architecture choices, their boundaries, and alternatives.
- [Labby product contracts over private Depot](./adr/0002-labby-product-contracts-over-private-depot.md) — accepted ownership and shared-shim decision, with a [current capability audit](./design/depot-capability-audit.md) and [direct control-plane implementation specification](./design/labby-depot-direct-control-plane.md).
- [Technology](./TECH.md) — toolchain, dependencies, build posture, Rustdoc, and release model.
- [Conventions](./CONVENTIONS.md) — engineering rules that current code is expected to follow.
- [Service documentation index](./services/README.md) and [service model](./dev/SERVICES.md) — product behavior plus the current registered-service and onboarding contracts.
- [CLI](./surfaces/CLI.md), [HTTP API](./surfaces/HTTP_API.md), [MCP](./surfaces/MCP.md), [MCP conformance](./surfaces/MCP_CONFORMANCE.md), and [Transport](./surfaces/TRANSPORT.md) — public surface behavior and protocol contracts.
- [Skills and Loadouts](./guides/SKILLS_AND_LOADOUTS.md) — Agent Skills trust/exposure and route Loadout projections.
- [Claude and Codex Artifact authoring](./artifacts/provider-authoring-reference.md) — official provider formats/frontmatter, sidecars, manifests, validation rules, and Creator mapping.
- [Phoenix Assistant](./services/PHOENIX_ASSISTANT.md) — App Server chronology, reasoning/tool event timeline, attachments, context usage, and verification oracles.
- [Local access bootstrap](./guides/LOCAL_ACCESS_BOOTSTRAP.md) — offline proof preparation, direct-local consume, recovery, revocation, and cleanup.
- [Access Control, Workspaces, and Artifact Distribution](./access-control/README.md) — active specification/contract for organizations, groups, projects, effective workspaces, scoped assets/capabilities, and Personal Labby Artifact sync/fork flows.
- [Skills-over-MCP compatibility](./plans/skills-over-mcp-compat/README.md) — historical implementation plan and progress record; the current contract is [Skills extension](./contracts/skills-extension.md) and current product behavior is [Artifacts And Agent Skills](./services/SKILLS.md).
- [Verification and compliance](./dev/VERIFICATION.md) — implemented specification oracles, model/replay tiers, real-process conformance, evidence boundaries, and qualification rules; operational commands live in the [verification workspace](../tools/verification/README.md).
- [First-use release qualification](./dev/FIRST_USE_RELEASE_QUALIFICATION.md) — timed clean-machine evidence, supported starting conditions, and remaining standalone trust boundary.
- [Discover MCP connection metadata](./contracts/discover-mcp-connection.md) — supported revision metadata and runtime verification.
- [Configuration](./runtime/CONFIG.md) and [Environment](./runtime/ENV.md) — runtime configuration and environment variables.
- [Operations](./OPERATIONS.md) — build, doctor, deployment, CI, release, and operator workflows.
- [Privilege-exposure runbook](./runtime/PRIVILEGE_EXPOSURE_RUNBOOK.md) — tamper review, credential rotation, owner re-verification, and config rollback after an admin-scope exposure.

## Current Product Services

The generated [service catalog](./generated/service-catalog.md) is authoritative. The current product documentation is split by service:

| Service | Product doc | Notes |
| --- | --- | --- |
| `access` | [services/ACCESS.md](./services/ACCESS.md) and [access-control/](./access-control/) | Principals, Teams, invitations, platform administration, onboarding, and owner recovery; design packet and data model |
| `projects` | [services/ACCESS.md#projects](./services/ACCESS.md#projects) | Team-scoped Project lifecycle: list, create, get, update, archive |
| `agents` | [services/AGENT_TASKS.md](./services/AGENT_TASKS.md) | Immutable Agent definitions executed through the shared Assistant LLM provider, bounded sessions, and revocation |
| `tasks` | [services/AGENT_TASKS.md](./services/AGENT_TASKS.md) and [services/TASKS.md](./services/TASKS.md) | Durable Agent Tasks, schedules, timezones, retries, and recovery |
| `browser` | [services/BROWSER.md](./services/BROWSER.md) | Rust-native WebMCP browser bridge, pairing, discovery, consent, and bounded invocation |
| `artifact_publish` | [services/ARTIFACT_PUBLISH.md](./services/ARTIFACT_PUBLISH.md) | Labby-owned protected archive publication workflow over configured Artifact authority |
| `dev_containers` | [services/DEV_CONTAINERS.md](./services/DEV_CONTAINERS.md) | Owner-scoped development-container definitions, leases, recovery, and lifecycle |
| `doctor` | [services/DOCTOR.md](./services/DOCTOR.md) | Always-on system, auth, OAuth relay, and proxy diagnostics |
| `gateway` | [services/GATEWAY.md](./services/GATEWAY.md) | Upstream catalog, protected routes, virtual servers, OAuth, Code Mode host |
| upstream proxy runtime | [services/UPSTREAM.md](./services/UPSTREAM.md) | HTTP/Unix/stdio upstream MCP connections, discovery, filtering, health, OAuth, skills |
| `setup` | [services/SETUP.md](./services/SETUP.md) | Bootstrap, settings, repair, proxy setup, host provisioning |
| `server_logs` | [services/SERVER_LOGS.md](./services/SERVER_LOGS.md) | Labby's own server-process log query and journal tail |
| `fs` | [services/FILESYSTEM.md](./services/FILESYSTEM.md) | Optional jailed read-only workspace browsing and preview |
| `stash` | [services/STASH.md](./services/STASH.md) | Linux principal-scoped file upload, download, sharing, and bounded MCP reads |
| `snippets` | [services/SNIPPETS.md](./services/SNIPPETS.md) | Reusable Code Mode workflow storage, validation, execution, testing, promotion |
| `artifacts`, `bundles`, `jobs`, `sources`, `uploads` | [services/SKILLS.md](./services/SKILLS.md) and [artifacts/](./artifacts/) | Durable Artifact library, provider-backed control-plane projections, and native Agent Skills projection |
| `lab_admin` | [services/LAB_ADMIN.md](./services/LAB_ADMIN.md) | Runtime-conditional onboarding audit surface |
| access owner bootstrap | [services/ACCESS.md](./services/ACCESS.md#owner-bootstrap) | Explicit creation of the first access-control owner (browser or offline proof); distinct from the registered `access` service |
| `proxy` | [guides/STDIO_MCP_PROXY.md](./guides/STDIO_MCP_PROXY.md) | Direct stdio proxy service: one selected stdio MCP server exposed over Streamable HTTP |

Do not hand-maintain a duplicate action inventory in prose. Use the generated [action catalog](./generated/action-catalog.md) for exact action names, parameters, scopes, destructive classification, and surfaces.

The [access owner bootstrap workflow](./services/ACCESS.md#owner-bootstrap)
has browser and direct-local proof flows. Its HTTP routes are not a separate
registered multi-surface service.

## Experimental connections

- [Tailcat browser connection](./guides/TAILCAT_BROWSER.md) — opt-in native pairing and authenticated Depot setup; full VM acceptance remains pending.

## Public Surfaces

- [CLI](./surfaces/CLI.md) — command grammar, output modes, confirmation behavior, and operator commands.
- [HTTP API](./surfaces/HTTP_API.md) — hand-authored HTTP transport contracts that supplement the generated route inventory.
- [MCP](./surfaces/MCP.md) — tool/resource/prompt behavior, Code Mode, MCP Apps, and capability exposure.
- [MCP conformance](./surfaces/MCP_CONFORMANCE.md) — current protocol-version and conformance contract.
- [RMCP](./surfaces/RMCP.md) — how Labby integrates the Rust MCP SDK.
- [Transport](./surfaces/TRANSPORT.md) — stdio, Streamable HTTP, Unix socket, middleware, CORS, DNS-rebinding protection, and subscriptions.

## Runtime And Operations

- [Configuration](./runtime/CONFIG.md)
- [Environment](./runtime/ENV.md)
- [OAuth](./runtime/OAUTH.md)
- [OAuth callback relay](./runtime/CALLBACK_RELAY.md)
- [Reverse proxy](./runtime/REVERSE_PROXY.md)
- [Host gateway runtime](./runtime/HOST_GATEWAY.md)
- [Incus](./runtime/INCUS.md)
- [Incus development-container provisioning](./runtime/INCUS_DEV_CONTAINER_PROVISIONING.md)
- [Unraid plugin](./runtime/UNRAID.md)
- [GitHub Actions runner](./runtime/ACTIONS_RUNNER.md)
- [CI/CD](./runtime/CICD.md)
- [Durable-state disaster recovery](./runtime/DISASTER_RECOVERY.md)
- [Operations](./OPERATIONS.md)
- [Technology and Rust build](./TECH.md)

## Developer Contracts

- [Snippet testing](./dev/SNIPPET_TESTING.md): offline fixtures, explicit live mode, resource budgets, and regression checks.

- [Dispatch](./dev/DISPATCH.md) — surface-neutral operation ownership and dependency direction.
- [Service model](./dev/SERVICES.md) — service inventory and registration rules.
- [Service onboarding](./dev/SERVICE_ONBOARDING.md) — end-to-end checklist for a new first-class capability.
- [Code Mode](./dev/CODE_MODE.md) — Code Mode runtime and host integration.
- [Errors](./dev/ERRORS.md) — stable error taxonomy and surface mapping.
- [Observability](./dev/OBSERVABILITY.md) — required fields, correlation, redaction, and verification.
- [Testing](./dev/TESTING.md) — local and CI verification expectations.
- [Verification and compliance](./dev/VERIFICATION.md) — qualification denominator, evidence classes, formal methods, and release verification ownership.
- [Rustdoc](./dev/RUSTDOC.md) — comprehensive Rust API documentation, doctest, and CI artifact contract.
- [Serialization](./design/SERIALIZATION.md) — output and wire-shape ownership.

Normative cross-surface contracts live under [contracts/](./contracts/):

- [Agent error contract](./contracts/agent-error-contract.md)
- [Integration identity](./contracts/integration-identity-v1.md) — authenticated installation and mounted-service discovery without credential-cache authority.
- [Code Mode tool errors](./contracts/code-mode-tool-errors.md)
- [MCP tool output](./contracts/mcp-tool-output.md)
- [Gateway schema resources](./contracts/gateway-schema-resources.md)
- [Skills extension](./contracts/skills-extension.md)
- [Stdio MCP proxy](./contracts/stdio-mcp-proxy.md)
- [Unraid Core integration](./contracts/unraid-core-integration-v1.md) —
  implemented unbundled appliance boundary; not an authorization to package
  Labby.
- [Core provider protocol](./contracts/core-provider-protocol-v1.md) —
  implemented private Core capability boundary for Labby Code Mode.

## Product Design

- [Gateway Admin design profile](./DESIGN.md) — self-contained Labby consumer profile for Aurora.
- [Design index](./design/README.md)
- [Labby and Depot SaaS North Star](./design/labby-depot-saas-north-star.md) — accepted target for public Depot, personal and hosted Labby, Lime as the first private tenant, paid team runtimes, R2, and the long-term tenant-aware data plane.
- [Phabby shared control plane](./design/phabby-control-plane.md) — accepted Phoenix/OTP target and Rust/BEAM ownership boundary.
- [Phabby migration ledger](./design/phabby-migration-ledger.md) — staged route, packaging, and ownership migration gates.
- [Web design-system contract](./design/design-system-contract.md)
- [Component development](./design/component-development.md)
- [CLI design system](./design/CLI_DESIGN_SYSTEM.md)
- [Claude Code Aurora theme](./design/CLAUDE_CODE_AURORA_THEME.md)
- [Google credential broker](./design/GOOGLE_CREDENTIAL_BROKER.md)
- [Remote gateway target](./design/REMOTE_GATEWAY_TARGET.md)
- [Unified Labby and Depot frontend](./depot-unified-frontend.md) — implemented frontend/runtime boundary and rollback contract for optional Depot integration.
- [Brand assets](./assets/brand/README.md)

## Maintained Plans And Implementation Records

These are useful engineering records, not substitutes for current product contracts:

- [Feature docs](./features/README.md) — proposed or in-flight behavior that must state explicitly when it is not shipped.
- [W19 Artifact Gateway record](./artifacts/progress.md) — historical Phase 1/2 lifecycle/provider milestone; current lifecycle behavior is [Artifacts And Agent Skills](./services/SKILLS.md).
- [Skills-over-MCP compatibility](./plans/skills-over-mcp-compat/README.md) — completed implementation record for PR #456; current protocol contract is [Skills extension](./contracts/skills-extension.md).
- [Provider-neutral Skills core](./plans/provider-neutral-skills-core/README.md) — completed implementation record for PR #486; current runtime behavior is projected through [Artifacts And Agent Skills](./services/SKILLS.md).
- [First-class usage metrics](./plans/usage-metrics-first-class/README.md) — shipped dimensional analytics record with its remaining long-window policy decision called out explicitly.
- [Verification and compliance](./dev/VERIFICATION.md) — current correctness-engineering architecture, qualification boundaries, and evidence contracts; operational usage lives in the [verification workspace](../tools/verification/README.md).

## Plugins And Snippets

- [Plugins](./PLUGINS.md) — separate installer and usage plugins, client registration, distribution boundaries, and binary-owned setup lifecycle.
- [Snippet authoring](./snippets/README.md) — executable Code Mode snippet format and workflow.

## Generated Product References

Run:

```bash
just docs-generate
just docs-check
```

Generated artifacts include:

- [service catalog](./generated/service-catalog.md)
- [action catalog](./generated/action-catalog.md)
- [environment reference](./generated/env-reference.md)
- [proxy configuration reference](./generated/proxy-config-reference.md)
- [API routes](./generated/api-routes.md)
- [MCP help](./generated/mcp-help.md)
- [CLI help](./generated/cli-help.md)
- [CLI migration](./generated/cli-migration.md)
- [feature matrix](./generated/feature-matrix.md)
- [OpenAPI](./generated/openapi.json)

Never edit generated artifacts by hand, including their README. The [generated ownership index](./generated/README.md) is derived from the artifact manifest and links each output to its source entrypoint. Correct the owning metadata, schema, Clap definition, or renderer before regenerating.

## Source-Of-Truth Rules

When documentation and implementation disagree:

1. verify the current implementation and generated catalogs;
2. fix the canonical product doc that owns the concern;
3. regenerate code-owned docs when code metadata changed;
4. update cross-links only where the behavior crosses product boundaries.

Avoid creating duplicate top-level summaries for a topic that already has a canonical service, runtime, surface, design, or developer doc.
