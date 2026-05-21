---
date: 2026-05-10 07:05:43 EST
repo: git@github.com:jmagar/lab.git
branch: main
head: 38c8397d
agent: Codex
session id: c7f3c5ad-9a4d-489b-8768-ed4d125abf5a
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/c7f3c5ad-9a4d-489b-8768-ed4d125abf5a.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab                                               38c8397d [main]
---

# MCP Server Debugging Session

## User Request

Investigate MCP startup failures for `axon` and `syslog`, then continue with a separate `swag-mcp` investigation using systematic debugging. Save the session to markdown.

## Session Overview

- Used the `superpowers:systematic-debugging` workflow to avoid guessing and trace each failure through config, credentials, HTTP metadata, service logs, and Docker state.
- Determined that `axon` and `syslog` were not down; their failures matched OAuth refresh/discovery behavior after cached access tokens expired.
- Determined that `swag-mcp` was not suffering the same OAuth metadata parse issue; it was temporarily unavailable during a Docker Compose rebuild/recreate.
- No source code or runtime config was intentionally modified during the investigations.

## Sequence of Events

- Read the systematic debugging skill instructions and checked prior local memory for related MCP/OAuth failures.
- Inspected `/home/jmagar/.codex/config.toml` to compare the configured MCP URLs for `axon`, `syslog`, `lab`, and `swag`.
- Probed public MCP endpoints and OAuth metadata URLs with `curl`.
- Checked Codex credential metadata in `/home/jmagar/.codex/.credentials.json` without reporting secret token values.
- Queried Codex logs in `/home/jmagar/.codex/logs_2.sqlite`.
- Read Docker process/container/log evidence for `swag-mcp`, including `docker logs`, `docker inspect`, and `docker events`.
- Confirmed `swag-mcp` recovered by performing an authenticated MCP `initialize` request with the cached Codex bearer token.

## Key Findings

- `axon` and `syslog` were configured in Codex as path-based MCP resources: `https://mcp.tootie.tv/axon` and `https://mcp.tootie.tv/syslog`, each with a matching `oauth_resource`.
- Their unauthenticated MCP `initialize` requests returned expected `401` JSON with `WWW-Authenticate` and protected-resource metadata, not a dead endpoint.
- Their protected-resource metadata advertised `authorization_servers: ["https://lab.tootie.tv"]`, and the canonical authorization server metadata at `https://lab.tootie.tv/.well-known/oauth-authorization-server` returned JSON.
- Service-scoped authorization-server URLs under `https://mcp.tootie.tv/.well-known/oauth-authorization-server/{axon,syslog}` returned the Labby HTML app, matching the class of "failed to parse server response" errors if a client derives that URL.
- Current Codex credentials for the path-based `axon` and `syslog` entries had expired access tokens, forcing refresh.
- `swag` was configured directly as `https://swag.tootie.tv/mcp` with no `oauth_resource` override.
- `swag` OAuth metadata was valid JSON at both `https://swag.tootie.tv/.well-known/oauth-protected-resource` and `https://swag.tootie.tv/.well-known/oauth-authorization-server`.
- `swag-mcp` Docker logs showed uvicorn shutdown and cancelled active SSE tasks during a container recreate; Cloudflare returned `502` during that backend gap.

## Technical Decisions

- Treated OAuth metadata, token freshness, and backend health as separate boundaries rather than assuming all MCP startup failures had the same cause.
- Avoided editing `/home/jmagar/.codex/.credentials.json` during diagnosis because the user asked to identify the issues, not reset credentials.
- Used live HTTP probes and Docker events as the decisive evidence for `swag-mcp` because service logs alone could not prove whether Cloudflare was failing before or after the backend.

## Files Modified

- `docs/sessions/2026-05-10-mcp-server-debugging.md` - saved this session note.

## Commands Executed

- `rg -n "swag|SWAG|MCP startup|oauth|OAuth token refresh|mcp.tootie.tv/swag" /home/jmagar/.codex/memories/MEMORY.md` - found prior SWAG/OAuth metadata fallthrough context.
- `rg -n "\[mcp_servers\.swag\]|url =|oauth_resource" /home/jmagar/.codex/config.toml` - confirmed Codex MCP server URL configuration.
- `curl -sS -i -X POST https://mcp.tootie.tv/axon ... initialize` - confirmed unauthenticated `axon` MCP returns `401` JSON.
- `curl -sS -i -X POST https://mcp.tootie.tv/syslog ... initialize` - confirmed unauthenticated `syslog` MCP returns `401` JSON.
- `curl -sS https://mcp.tootie.tv/.well-known/oauth-protected-resource/{axon,syslog}` - confirmed protected-resource metadata advertised `https://lab.tootie.tv`.
- `curl -sS -i https://mcp.tootie.tv/.well-known/oauth-authorization-server/{axon,syslog}` - found HTML instead of OAuth JSON at the service-scoped auth-server URLs.
- `curl -sS https://lab.tootie.tv/.well-known/oauth-authorization-server` - confirmed the canonical Lab authorization server metadata returned JSON.
- `docker logs --since 3h swag-mcp` - showed `swag-mcp` MCP requests before restart and uvicorn shutdown/restart evidence.
- `docker events --since '2026-05-10T02:45:00Z' --until '2026-05-10T02:53:00Z' --filter container=swag-mcp ...` - showed `kill`, `stop`, `die`, `destroy`, `create`, and `start` events for `swag-mcp`.
- `curl -sS -i -X POST https://swag.tootie.tv/mcp ... initialize` with the cached bearer token - first hit Cloudflare `502` during recreate, then returned `HTTP/2 200` with a valid MCP `initialize` result after restart.

## Errors Encountered

- A first Docker log read failed with a Docker socket permission error; rerunning with the approved Docker log command succeeded.
- A SQLite query with shell backticks around `swag` caused shell command substitution noise; later queries avoided relying on that pattern.
- A `jq` query initially assumed `.credentials.json` had an `mcp.oauth_tokens` shape; the actual file is keyed by server entry at the top level.
- The `swag-mcp` public endpoint returned Cloudflare `502` while the backend container was being recreated; it resolved once the new container became healthy.

## Behavior Changes (Before/After)

- No behavior was intentionally changed in this session.
- Before investigation, `axon`, `syslog`, and `swag` had similar-looking MCP startup symptoms.
- After investigation, `axon` and `syslog` were classified as OAuth refresh/discovery issues, while `swag` was classified as a transient backend recreate/unavailability issue.

## Verification Evidence

| command | expected | actual | status |
| --- | --- | --- | --- |
| `curl -sS https://lab.tootie.tv/.well-known/oauth-authorization-server` | OAuth authorization server JSON | JSON with issuer `https://lab.tootie.tv` and token endpoint `/token` | pass |
| `curl -sS -i https://mcp.tootie.tv/.well-known/oauth-authorization-server/axon` | OAuth JSON if client derives service-scoped metadata | `HTTP/2 200` HTML Labby app | fail, explains parse-risk |
| `curl -sS -i https://mcp.tootie.tv/.well-known/oauth-authorization-server/syslog` | OAuth JSON if client derives service-scoped metadata | `HTTP/2 200` HTML Labby app | fail, explains parse-risk |
| `curl -sS -i https://swag.tootie.tv/.well-known/oauth-authorization-server` | OAuth JSON | JSON from `https://mcp-auth.tootie.tv` metadata | pass |
| `curl -sS -i http://127.0.0.1:8012/health` | Local `swag-mcp` health is healthy | `HTTP/1.1 200 OK` with `{"status":"healthy"}` | pass |
| Authenticated `initialize` to `https://swag.tootie.tv/mcp` after restart | MCP initialize succeeds | `HTTP/2 200` event stream with `serverInfo.name = "SWAG Configuration Manager"` | pass |
| `docker inspect swag-mcp` | Running healthy container after recreate | status `running`, health `healthy`, started `2026-05-10T02:52:32Z` | pass |

## Risks and Rollback

- No runtime or source changes were made, so there is no code rollback.
- If credential reset is attempted later for `axon` or `syslog`, back up `/home/jmagar/.codex/.credentials.json` first and remove only the specific affected MCP entries.
- If `swag-mcp` restarts during active MCP sessions, FastMCP SSE requests can be cancelled and public clients can see Cloudflare `502` until the replacement container is ready.

## Decisions Not Taken

- Did not delete or regenerate Codex OAuth credentials for `axon`, `syslog`, or `swag`.
- Did not change reverse-proxy metadata routes for `mcp.tootie.tv` or `swag.tootie.tv`.
- Did not restart `swag-mcp`; the observed recreate was already caused by an active Docker Compose rebuild process.

## References

- `/home/jmagar/.codex/config.toml`
- `/home/jmagar/.codex/.credentials.json`
- `/home/jmagar/.codex/logs_2.sqlite`
- `https://mcp.tootie.tv/.well-known/oauth-protected-resource/axon`
- `https://mcp.tootie.tv/.well-known/oauth-protected-resource/syslog`
- `https://lab.tootie.tv/.well-known/oauth-authorization-server`
- `https://swag.tootie.tv/.well-known/oauth-protected-resource`
- `https://swag.tootie.tv/.well-known/oauth-authorization-server`

## Open Questions

- Whether Codex/rmcp is actually deriving `https://mcp.tootie.tv/.well-known/oauth-authorization-server/{service}` during refresh was inferred from the parse failure and live endpoint behavior; the Codex logs inspected did not show the exact metadata URL used internally.
- The session identified an active `docker compose up -d --build` process for `swag-mcp`, but did not trace which user action or automation started it.
- The repo already had unrelated dirty files before saving this note; those changes were not investigated in this session.

## Next Steps

Started but not completed:

- None.

Follow-on tasks not yet started:

- For `axon` and `syslog`, either clear only the affected Codex credential entries to force fresh auth or make the service-scoped `mcp.tootie.tv/.well-known/oauth-authorization-server/{axon,syslog}` routes return valid OAuth JSON.
- For `swag-mcp`, consider adding deployment/recreate coordination or health-aware retry guidance so MCP clients do not start during a compose rebuild gap.
- Review why `swag-mcp` logs `SWAG_MCP_NO_AUTH=true` and confirm the proxy/auth boundary is intentionally the only public enforcement layer.
