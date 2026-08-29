---
title: Palette integration profile
created: 2026-08-29
updated: 2026-08-29
---

# Palette integration profile

Labby owns this contract. The canonical source is `contracts/integration-profile.schema.json`; the generated consumer snapshot is `docs/contracts/generated/integration-profile.schema.json`; check both plus fail-closed fixtures with `python3 scripts/check-integration-contracts.py`.

This root exposes only stable product identity, API compatibility, auth binding, capability names, and stream support. Exact-call, catalog, loadout, snippet, and artifact DTOs belong to their delivery slices and must extend Labby's own OpenAPI/schema generation rather than this profile or a cross-product DTO package.

Credentials are bound to profile, canonical origin, `server_id`, issuer, audience, token endpoint origin, principal cache scope, and credential generation. Any change requires explicit re-trust. Authenticated API/SSE redirects are rejected; discovery is credential-free and its final origin is pinned. Axon-to-Labby service identity uses a dedicated audience and principal scope and is never interchangeable with a user's Palette credential.

Cache keys include stable server identity, API major, principal/auth snapshot, credential generation, catalog/capability generation, object revision, query digest, and cursor lineage. Owners must specify TTL, byte/item cap, stale policy, and synchronous invalidation. Performance traces cover catalog page/detail, loadout resolution, approval, exact call, stream connect/resume, IPC, and render commit using bounded labels; credentials, principals, tool arguments, schemas, and artifact content are redacted.
