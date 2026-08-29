---
title: "Public OAuth Callback Relay Cutover"
created: "2026-07-30"
updated: "2026-07-30"
---

# Public OAuth Callback Relay Cutover

This runbook moves `https://callback.tootie.tv/callback/{machine_id}[/{suffix}]`
from the standalone Python `callback-relay` container on `edgehost` into Labby.

The Labby relay is transport-only. It forwards the final OAuth callback request
to the registered machine target. Codex or the MCP client still owns PKCE,
`state`, and token exchange.

## Client Behavior

Regular non-headless desktop clients should keep local loopback callbacks.

Remote, headless, or cross-namespace clients may use:

```toml
mcp_oauth_callback_url = "https://callback.tootie.tv/callback/<machine>"
```

Valid Labby public relay targets look like:

```text
http://100.99.0.1:38935/callback/<machine>
```

The host must be a concrete Tailscale CGNAT address in `100.64.0.0/10` (that
range describes the *allowed network*, not a valid URL hostname on its own).
Targets with HTTPS, non-38935 ports, userinfo, query strings, fragments,
loopback, link-local, or non-Tailscale IPs are rejected.

## Preflight

Every transition uses the run-owned end-to-end canary. Export the admin bearer
without placing it on the command line, and substitute this host's concrete
Tailscale address for `--target-host`:

```bash
export LABBY_ADMIN_BEARER_TOKEN='...'
run_canary() {
  python3 scripts/oauth_relay_cutover_canary.py \
    --public-base https://callback.tootie.tv \
    --admin-base https://labby.example.com \
    --target-host 100.99.0.1 "$@"
}
```

The canary owns a unique registry identity, listener, code, and state. It
requires exact callback delivery and response propagation, then removes the
identity and confirms it is absent. Any failure, timeout, skipped check, or
cleanup residue exits nonzero and blocks the transition. The bearer value is
never printed.

Verify Labby is reachable through SWAG:

```bash
ssh edgehost 'docker exec swag curl -fsS --max-time 5 http://100.99.0.1:40100/health'
```

Export the current standalone relay registry:

```bash
ssh edgehost 'docker exec callback-relay cat /app/.cache/callback-relay/registry.json' > /tmp/callback-relay-registry.json
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

Prove the currently serving relay before changing SWAG:

```bash
run_canary --phase pre-cutover
```

Do not continue unless the JSON result is `status=passed`,
`machine_removed=true`, `exact_delivery=true`, and `exact_response=true`.

## Cutover

Update the SWAG `callback.tootie.tv` upstream from `callback-relay:39001` to the
Labby HTTP service on devhost.

Validate the SWAG config and reload:

```bash
ssh edgehost 'docker exec swag nginx -t'
ssh edgehost 'docker exec swag nginx -s reload'
```

Use shallow health only as an initial diagnostic:

```bash
curl -fsS --max-time 5 https://callback.tootie.tv/healthz
```

Expected shape:

```json
{"status":"ok","relay":"enabled","registry":"loaded","machines":7}
```

Run the registry/target doctor, then require the end-to-end post-cutover canary:

```bash
labby doctor oauth-relay --probe-targets --json
run_canary --phase post-cutover
```

If either command fails, immediately restore the old SWAG upstream. A shallow
`/healthz` success never authorizes completion. Before production cutover,
exercise staging failures at SWAG routing, registry lookup, target reachability,
and response forwarding; each injected failure must make the canary fail and
trigger this rollback decision.

## Rollback

Restore the SWAG upstream for `callback.tootie.tv` to:

```text
callback-relay:39001
```

Then validate and reload SWAG:

```bash
ssh edgehost 'docker exec swag nginx -t'
ssh edgehost 'docker exec swag nginx -s reload'
```

Recheck the public endpoint after rollback:

```bash
curl -fsS --max-time 5 https://callback.tootie.tv/healthz
run_canary --phase post-rollback
```

If rollback points back to the Python relay, use the standalone relay's own
health behavior only as an initial diagnostic. Rollback is complete only after
the same end-to-end canary passes and its run-owned registry entry is absent.
