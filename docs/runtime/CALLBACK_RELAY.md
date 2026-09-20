---
title: "Public OAuth Callback Relay Operations"
created: "2026-07-30"
updated: "2026-09-16"
status: "deployed"
---

# Public OAuth Callback Relay Operations

The public callback-relay cutover described by the original July runbook has
already occurred. Current SWAG configuration on SQUIRTS routes
`https://callback.tootie.tv` to the Labby gateway on Dookie at
`10.1.0.6:40100`. The legacy Python `callback-relay` container is still
running on SQUIRTS as the explicit rollback target.

The Labby relay is transport-only. It forwards the final OAuth callback request
to the registered machine target. Codex or the MCP client still owns PKCE,
`state`, and token exchange.

> **Point-in-time verification, 2026-09-16:** the deployed SWAG configuration is
> still pointed at Labby, but a direct SQUIRTS-to-`10.1.0.6:40100` probe could
> not connect and the public `/healthz` endpoint returned HTTP 502. Treat that
> as a current infrastructure incident, not as the expected relay contract.
> Re-run the checks below after Dookie/Labby service recovery. The legacy
> `callback-relay` container was still running at the time of this audit.

## Current Topology

- Public hostname: `callback.tootie.tv`.
- Reverse proxy: SWAG on SQUIRTS.
- Current upstream: Labby on Dookie, `http://10.1.0.6:40100`.
- Rollback upstream: `callback-relay:39001`.
- Public relay health path: `/healthz`.
- Deep registry/target check: `labby doctor oauth-relay --probe-targets`.

The checked-in Labby implementation owns the public relay registry, forwarding
policy, admin surface, and doctor checks. SWAG owns only the public reverse-proxy
hop.

## Client Behavior

Regular non-headless desktop clients should keep local loopback callbacks.
Remote, headless, or cross-namespace clients may use:

```toml
mcp_oauth_callback_url = "https://callback.tootie.tv/callback/<machine>"
```

Valid public-relay targets use a concrete Tailscale CGNAT address in
`100.64.0.0/10`, for example:

```text
http://100.99.0.1:38935/callback/<machine>
```

The network range itself is not a valid hostname. Targets with HTTPS,
non-38935 ports, userinfo, query strings, fragments, loopback, link-local, or
non-Tailscale IPs are rejected by the relay policy.

## Routine Verification

Verify the current Labby upstream from the SWAG network. Preserve the public
Host header so Labby's callback-relay routing policy sees the same host used by
external traffic:

```bash
ssh squirts 'docker exec swag curl -fsS --max-time 5 -H "Host: callback.tootie.tv" http://10.1.0.6:40100/healthz'
```

Verify the public shallow endpoint:

```bash
curl -fsS --max-time 5 https://callback.tootie.tv/healthz
```

Expected healthy shape:

```json
{"status":"ok","relay":"enabled","registry":"loaded","machines":7}
```

The machine count is environment state, not a protocol constant.

Run the deep registry and target check from an authenticated Labby operator
context when reachability matters:

```bash
labby doctor oauth-relay --probe-targets --json
```

A failing public check and a failing direct-upstream check indicate a Labby or
Dookie reachability problem. A healthy direct check with a failing public check
points toward SWAG/DNS/TLS. A healthy shallow check with target-probe failures
points toward relay registry entries or per-machine callback listeners.

## Registry Operations

Inspect current registry state before mutating it:

```bash
labby oauth relay-registry list --json
```

The CLI also supports add/update, enable/disable, remove, and whole-registry
import operations. Whole-registry import replaces the active registry and is a
destructive operator action; do not use the legacy container as a routine source
of truth after cutover.

The original migration used this one-time export/import path:

```bash
ssh squirts 'docker exec callback-relay cat /app/.cache/callback-relay/registry.json' > /tmp/callback-relay-registry.json
labby oauth relay-registry import --file /tmp/callback-relay-registry.json --json
```

Use that only for an intentional rebuild/recovery when the legacy registry is
known to be authoritative. A failed import is all-or-nothing.

## SWAG Configuration

The live SWAG proxy configuration audited on 2026-09-16 resolves the public
callback host to:

```text
10.1.0.6:40100
```

Validate and reload SWAG after any intentional proxy change:

```bash
ssh squirts 'docker exec swag nginx -t'
ssh squirts 'docker exec swag nginx -s reload'
```

Do not repeat the old cutover procedure merely because the legacy
`callback-relay` container is still running. Its continued presence is the
rollback mechanism.

## Rollback

Rollback is appropriate when Labby cannot serve the public relay and the legacy
container is known healthy. Restore the SWAG upstream to:

```text
callback-relay:39001
```

Then validate/reload SWAG and re-run the public health probe. If rollback points
back to the Python relay, that relay's own health behavior is authoritative until
traffic is cut back to Labby.

After Labby is healthy again, re-qualify the direct upstream, public `/healthz`,
and `labby doctor oauth-relay --probe-targets --json` before returning the
public proxy to Labby.
