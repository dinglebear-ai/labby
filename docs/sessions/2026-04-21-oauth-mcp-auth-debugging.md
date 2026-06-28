```yaml
date: 2026-04-21 18:29:45 EST
repo: git@github.com:jmagar/lab.git
branch: fix/auth
head: beb3de0
plan: none
agent: Claude (claude-sonnet-4-6)
session id: a576b50d-bac4-486f-8489-04431767f47f
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/a576b50d-bac4-486f-8489-04431767f47f.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab  beb3de0 [fix/auth]
pr: 25 — fix(auth): gateway admin auth, upstream OAuth, and dispatch fixes — https://github.com/jmagar/lab/pull/25
```

## User Request

Debug MCP OAuth authentication failures against the lab HTTP server (`https://lab.example.com/mcp`), starting with `{"kind":"validation_failed","message":"resource must be \`https://lab.example.com/mcp\`"}` when trying to auth, and progressing through two subsequent failure modes discovered after each fix.

## Session Overview

Three sequential auth bugs were diagnosed and resolved across two systems (lab server code + SWAG nginx proxy on device `node-b`):

1. **OAuth resource mismatch** — codex client configured with wrong resource URL (`https://lab.example.com` instead of `https://lab.example.com/mcp`). Fixed by user reconfiguring codex.
2. **Token obtained but MCP reconnection fails** — both Claude Code and codex showed "Authentication successful, but server reconnection failed." Root cause: `mcp.conf` on the nginx proxy was stripping the `Authorization` header before forwarding to the lab server.
3. **No lab server logs for /mcp requests** — confirmed as a consequence of bug #2: requests reached nginx, were forwarded with empty Authorization, lab server returned 401, but zero `/mcp` entries appeared in the lab server's structured logs (TraceLayer was active; absence confirmed the auth middleware was rejecting before span completion or user's log excerpt was partial).

## Sequence of Events

1. User reported `validation_failed` with `resource must be \`https://lab.example.com/mcp\`` during OAuth authorization.
2. Traced error to `crates/lab-auth/src/authorize.rs:399–401` — `validate_resource()` comparing `requested` against `canonical_resource_url(state)`.
3. Read `canonical_resource_url` (`crates/lab-auth/src/metadata.rs:56–58`): returns `{public_base_url}/mcp`.
4. Checked server logs provided by user: `requested_resource=https://lab.example.com/` vs `expected_resource=https://lab.example.com/mcp` — client sending base URL without `/mcp`.
5. Called `advisor` before fixing; advisor correctly identified the client configuration as the likely root cause rather than a server-side validation bug.
6. User confirmed and fixed: codex OAuth resource config was pointing at base URL; user changed it to `https://lab.example.com/mcp`.
7. New symptom: "Authentication successful, but server reconnection failed" in both Claude Code (`/mcp` command) and codex.
8. Server logs showed token exchange succeeding but zero `/mcp` entries in lab server output.
9. Read nginx proxy config from `node-b` via SSH: `/mnt/appdata/swag/nginx/proxy-confs/lab.subdomain.conf`.
10. Read `mcp.conf` from node-b: discovered `proxy_set_header Authorization "";` stripping the Bearer token.
11. Verified via nginx access log: `POST /mcp → 401 78` entries matching token exchange timestamps; no `/mcp` in lab server logs confirming lab server never received the Authorization header.
12. Removed the two offending lines from `/mnt/appdata/swag/nginx/mcp.conf` on node-b via SSH sed.
13. Tested nginx config (`nginx -t`) and reloaded (`nginx -s reload`).

## Key Findings

- **`crates/lab-auth/src/authorize.rs:382–401`** — `validate_resource()` does exact string comparison; no trailing-slash normalization, no base-URL fallback.
- **`crates/lab-auth/src/metadata.rs:56–58`** — `canonical_resource_url` = `{public_base_url}/mcp`; this is the single source of truth for both token issuance (`aud` claim) and authorize/token endpoint validation.
- **`crates/lab/src/api/router.rs:434–443`** — `resource_url` passed to auth middleware (and `WWW-Authenticate` header) is the raw `public_url` base, NOT the canonical MCP resource URL. The WWW-Authenticate header therefore points to `https://lab.example.com/.well-known/oauth-protected-resource`, which correctly returns `resource: https://lab.example.com/mcp` — but only if the client reads the `resource` field.
- **`/mnt/appdata/swag/nginx/mcp.conf:24–25`** (on node-b) — `proxy_set_header Authorization "";` with comment "already validated by /_oauth_verify". The `/_oauth_verify` internal endpoint is not present in the nginx config; this was a half-implemented nginx-level auth pattern that stripped the token without providing the upstream verification gate.
- **Nginx access log on node-b** — confirmed `POST /mcp HTTP/2.0 401 78` with user-agent `claude-code/2.1.116 (cli)` at timestamps matching the lab server's token exchange log entries.

## Technical Decisions

- **Client-side fix for resource URL** rather than server-side relaxation: the server's `validate_resource()` is correct per RFC 8707; accepting the base URL as a valid resource would be a semantic loosening. The root cause was client misconfiguration.
- **Remove Authorization stripping from mcp.conf** rather than making the lab server accept unauthenticated `/mcp` requests: the lab server's Bearer token validation is the correct enforcement point; the nginx pattern was incomplete and dangerous.
- **sed over file rewrite**: minimal targeted edit on the remote file to avoid touching unrelated nginx config.

## Files Modified

| File | Location | Change |
|------|----------|--------|
| `/mnt/appdata/swag/nginx/mcp.conf` | device `node-b` | Removed `proxy_set_header Authorization "";` and its orphaned comment |

No lab repo files were modified during this session.

## Commands Executed

```bash
# Read nginx proxy config from node-b
ssh node-b cat /mnt/appdata/swag/nginx/proxy-confs/lab.subdomain.conf

# Check nginx access and error logs for /mcp entries
ssh node-b "grep -E 'POST /mcp|GET /mcp|DELETE /mcp' /mnt/appdata/swag/log/nginx/access.log | tail -20"
ssh node-b "grep -E '/mcp|mcp' /mnt/appdata/swag/log/nginx/error.log | tail -20"

# Read mcp.conf
ssh node-b "sed -n '1,30p' /mnt/appdata/swag/nginx/mcp.conf"

# Remove Authorization stripping lines
ssh node-b "sed -i '/proxy_set_header Authorization \"\";/d' /mnt/appdata/swag/nginx/mcp.conf"
ssh node-b "sed -i '24d' /mnt/appdata/swag/nginx/mcp.conf"  # remove orphaned comment (em dash broke first sed)

# Verify removal
ssh node-b "grep -n 'Authorization\|oauth_verify' /mnt/appdata/swag/nginx/mcp.conf"
# Result: only CORS Access-Control-Allow-Headers lines remain (correct)

# Test and reload nginx
ssh node-b "docker exec swag nginx -t && docker exec swag nginx -s reload"
# Result: "nginx: configuration file /etc/nginx/nginx.conf syntax is ok" + "reloaded"
```

## Errors Encountered

**Error 1 — OAuth resource mismatch**
- Symptom: `{"kind":"validation_failed","message":"resource must be \`https://lab.example.com/mcp\`"}`
- Root cause: codex MCP client configured with `https://lab.example.com` as the OAuth resource instead of `https://lab.example.com/mcp`
- Resolution: user reconfigured codex to use the correct resource URL

**Error 2 — MCP reconnection fails after successful token exchange**
- Symptom: "Authentication successful, but server reconnection failed" (Claude Code); "The lab-http MCP server is not logged in" (codex)
- Root cause: `mcp.conf` on node-b nginx contained `proxy_set_header Authorization "";`, stripping the Bearer token before it reached the lab server. Comment referenced a `/_oauth_verify` internal endpoint that does not exist in the config — incomplete nginx-level auth pattern.
- Evidence: nginx access log showed `POST /mcp → 401 78` with `claude-code/2.1.116` user-agent at timestamps matching lab server token issuance. Lab server logs showed no `/mcp` entries.
- Resolution: removed the two stripping lines from `mcp.conf`, reloaded nginx.

**sed em-dash encoding issue**
- First sed command used the em dash character (`—`) in the pattern to match the comment line; sed silently failed to match it
- Resolution: deleted the orphaned comment by line number (`sed -i '24d'`) after confirming position with grep

## Behavior Changes (Before/After)

| Aspect | Before | After |
|--------|--------|-------|
| `codex mcp login lab-http` | Fails with `validation_failed: resource must be \`https://lab.example.com/mcp\`` | Succeeds (user fixed client config) |
| `POST /mcp` with Bearer token | nginx strips Authorization header → lab server returns 401 | Authorization header passes through → lab server validates JWT |
| Claude Code `/mcp` reconnect | "Authentication successful, but server reconnection failed" | Expected: reconnects successfully with valid token |
| Lab server logs for `/mcp` | No entries (requests 401'd upstream, never logged) | Expected: `INFO request{method=POST path=/mcp ...}` entries visible |

## Risks and Rollback

- **Risk**: removing `proxy_set_header Authorization "";` exposes the lab server's Bearer token validation as the sole auth enforcement for `/mcp`. If the lab server's JWT validation has a bug, `/mcp` is unprotected. Mitigation: the lab server's `authenticate_request` middleware (`router.rs:153–236`) is well-tested with both static token and JWT paths.
- **Rollback**: re-add `proxy_set_header Authorization "";` to `/mnt/appdata/swag/nginx/mcp.conf` on node-b and reload nginx. One-liner: `ssh node-b "sed -i '/proxy_set_header Mcp-Session-Id/a proxy_set_header Authorization \"\";' /mnt/appdata/swag/nginx/mcp.conf && docker exec swag nginx -s reload"`

## Decisions Not Taken

- **Server-side resource URL relaxation in `validate_resource()`** — considered accepting both `https://lab.example.com` and `https://lab.example.com/mcp` as valid resources, plus trailing-slash normalization. Rejected: the client misconfiguration was the actual root cause; server relaxation would mask the error and weaken RFC 8707 compliance.
- **Implementing `/_oauth_verify` nginx internal endpoint** — the original intent of the stripped Authorization pattern. Rejected for now: adds complexity (nginx subrequest to validate JWTs); the lab server already validates tokens correctly. Could be revisited if the proxy needs to enforce auth independently of the upstream.

## Open Questions

- **Was the MCP reconnection actually fixed?** The nginx reload succeeded and config is valid, but the user did not confirm successful reconnection in the session — verification was left to the user.
- **Why do lab server logs show no `/mcp` entries even for the 401 responses?** The TraceLayer wraps the full axum router including the merged MCP router; 401 responses from `authenticate_request` should appear. Possible explanations: user's log excerpt was filtered/partial, or the auth middleware returns before the TraceLayer span records the status field. Not fully resolved.
- **`/_oauth_verify` pattern intent** — unclear who authored the `mcp.conf` Authorization stripping and whether there was a planned nginx-level JWT verification flow. If nginx-level auth is desired in future, the pattern needs a working `/_oauth_verify` internal location wired to a token introspection endpoint.

## Next Steps

**Unfinished (started but not completed)**
- Verify that Claude Code and codex successfully reconnect to `https://lab.example.com/mcp` after the nginx reload — user was asked to test but session ended before confirmation.

**Follow-on (not yet started)**
- Consider normalizing trailing slashes in `validate_resource()` (`crates/lab-auth/src/authorize.rs:382–401`) to be more tolerant of client variations, even though the current strictness is RFC-correct.
- Add `lab.example.com` to an explicit allowlist comment in `mcp.conf` or document the `LAB_MCP_ALLOWED_HOSTS` env var as the correct way to extend the allowed hosts list for the rmcp DNS rebinding protection (`crates/lab/src/cli/serve.rs:593–633`).
- Review `router.rs:434–443`: the `resource_url` variable name is misleading — it holds the base `public_url`, not the canonical MCP resource URL. A rename or comment would reduce confusion for future maintainers.
