# labby-gateway — Surface-Neutral Gateway Runtime

This crate owns Labby's reusable upstream MCP gateway runtime. It is not a CLI, HTTP API, or web application.

## Ownership

- `src/upstream/`: connection pool, transports, discovery, bounded listing, relay/callback routing, resource/prompt/tool proxying, health, tasks/skills
- `src/gateway/`: gateway manager, config mutation, virtual servers, protected routes, OAuth lifecycle, Code Mode host integration, view models
- `src/security/`: spawn and SSRF guards
- `src/usage/`: gateway usage records/querying
- `src/codemode_journal/`: Code Mode journal persistence helpers

Keep this crate surface-neutral. Do not add clap/axum product adapters or reach into `crates/labby` product registry builders.

Read the more specific `src/gateway/CLAUDE.md` or `src/upstream/CLAUDE.md` before changing those trees. Preserve bounded upstream enumeration and typed/recovery-aware errors.
