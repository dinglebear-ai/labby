---
title: Integration identity v1
created: 2026-09-04
updated: 2026-09-05
---

# Integration identity v1

`GET /v1/integration/identity` returns a non-authorizing snapshot of the local
daemon. It uses normal `/v1` authentication and is mounted only when a static
bearer or OAuth runtime is configured, the daemon initialized its installation
identity, and trusted-host integration is disabled. It is not a public discovery
endpoint and is unavailable in trusted-host mode in this version.
Responses use `Cache-Control: private, no-store` so installation metadata and
mounted-service snapshots are not reused by intermediaries across runtime changes.

The complete response is defined by [the JSON Schema](integration-identity-v1.schema.json).
`server_id` is a stable SHA-256 projection of the existing
`LABBY_HOME/installation-id`, not a second persisted identity. Startup resolves
the ID while holding the installation lifecycle lock; GET performs no filesystem
I/O. Missing IDs are published atomically without overwriting a concurrent
winner. Invalid or insecure existing IDs fail startup instead of being replaced.
Restoring or cloning an installation preserves its ID; this is installation
continuity, not proof of unique hardware or an authorization principal.

`capabilities` contains sorted, unique registered service names with mounted HTTP
routes. It does not list future loadouts or delegation, authorize any action,
advertise upstream health, or promise a configured Depot connection.

`auth.modes` reports only mounted static-bearer/OAuth credential modes. OAuth
`issuer` and `audience` come from the actual auth runtime's public URL and
canonical resource calculation. `token_endpoint_origin` is the URL origin,
without an issuer path. These values are null without OAuth. There is no invented
`/v1` audience or service-identity mode. `principal_cache_scope` and
`credential_generation` are explicitly null: this endpoint grants no principal
cache authority and does not expose or hash credentials. Callers must continue
normal authentication and cannot use discovery as a credential-validation cache.

`streams` describes this integration endpoint only: it exposes no event stream
or resumable event cursor. Other service-specific streaming APIs are unchanged.
