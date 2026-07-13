# Public OAuth Callback Relay Cutover

This runbook moves `https://callback.tootie.tv/callback/{machine_id}[/{suffix}]`
from the standalone Python `callback-relay` container on `squirts` into Labby.

The Labby relay is transport-only. It forwards the final OAuth callback request
to the registered machine target. Codex or the MCP client still owns PKCE,
`state`, and token exchange.

## Client Behavior

Regular non-headless desktop clients should keep local loopback callbacks.

Remote, headless, or cross-namespace clients may use:

```toml
mcp_oauth_callback_url = "https://callback.tootie.tv/callback/<machine>"
```

Valid Labby public relay targets are:

```text
http://100.64.0.0/10:38935/callback/<machine>
```

Targets with HTTPS, non-38935 ports, userinfo, query strings, fragments,
loopback, link-local, or non-Tailscale IPs are rejected.

## Preflight

Verify Labby is reachable through SWAG:

```bash
ssh squirts 'docker exec swag curl -fsS --max-time 5 http://100.88.16.79:40100/health'
```

Export the current standalone relay registry:

```bash
ssh squirts 'docker exec callback-relay cat /app/.cache/callback-relay/registry.json' > /tmp/callback-relay-registry.json
```

Import it into Labby's sidecar registry:

```bash
labby oauth relay-registry import --file /tmp/callback-relay-registry.json --json
```

The import is all-or-nothing. If any machine id or target URL is quarantined,
fix the exported file and rerun the import; Labby will not partially replace the
active registry.

Restart Labby after CLI-side registry import so the running server refreshes
its in-memory snapshot.

## Cutover

Update the SWAG `callback.tootie.tv` upstream from `callback-relay:39001` to the
Labby HTTP service on dookie.

Validate the SWAG config and reload:

```bash
ssh squirts 'docker exec swag nginx -t'
ssh squirts 'docker exec swag nginx -s reload'
```

Verify the public shallow health endpoint:

```bash
curl -fsS --max-time 5 https://callback.tootie.tv/healthz
```

Expected shape:

```json
{"status":"ok","relay":"enabled","registry":"loaded","machines":7}
```

Run an explicit deep check from an operator shell when target reachability
matters:

```bash
labby doctor oauth-relay --probe-targets --json
```

## Rollback

Restore the SWAG upstream for `callback.tootie.tv` to:

```text
callback-relay:39001
```

Then validate and reload SWAG:

```bash
ssh squirts 'docker exec swag nginx -t'
ssh squirts 'docker exec swag nginx -s reload'
```

Recheck the public endpoint after rollback:

```bash
curl -fsS --max-time 5 https://callback.tootie.tv/healthz
```

If rollback points back to the Python relay, use the standalone relay's own
health behavior as the authority for that check.
