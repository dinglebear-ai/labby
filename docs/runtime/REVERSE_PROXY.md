---
title: "Reverse Proxy Deployment"
created: "2026-07-30"
updated: "2026-09-05"
---

# Reverse Proxy Deployment

## Fast path

For a normal single-origin Browser + ChatGPT deployment, generate a validated
streaming-safe proxy configuration instead of starting from the examples below:

```bash
# Recommended: Caddy
labby setup public-proxy --public-url https://lab.example.com

# Advanced alternatives
labby setup public-proxy --public-url https://lab.example.com --format nginx
labby setup public-proxy --public-url https://lab.example.com --format traefik
```

The same `public_proxy.render` contract is exposed in **Settings → Doctor →
Public HTTPS**. It requires an HTTPS public origin, defaults the private backend
to `http://127.0.0.1:8765`, contains no secrets, and returns verification
commands. Caddy is the default because it minimizes first-run TLS/proxy
configuration. The remainder of this document covers advanced multi-host,
protected-route, edge-network, and manual proxy requirements.

Labby can serve the web UI, OAuth server, native `/mcp`, and Gateway-managed
protected MCP routes from the same HTTP listener. Put your reverse proxy in
front of that listener and configure public MCP routes in Labby.

## Model

- `LABBY_PUBLIC_URL` is the Labby app and OAuth issuer, for example `https://lab.example.com`.
- Each protected MCP route has its own public resource identity, for example `https://mcp.example.com/tools`.
- The reverse proxy forwards public hosts to Labby without rewriting the path.
- Labby matches `Host + path`, serves route-specific OAuth metadata, validates route-audience JWTs, and proxies accepted MCP traffic to the private backend.

## Required Proxy Behavior

- Preserve the original `Host` header.
- Set `X-Forwarded-Proto` to the original scheme.
- Forward `Authorization`, `Accept`, `Content-Type`, `Mcp-Protocol-Version`,
  and every SEP-2243 `Mcp-*` routing header (`Mcp-Method`, `Mcp-Name`, and
  `Mcp-Param-*`).
- Disable request and response buffering on MCP paths.
- Disable compression on MCP paths.
- Use read/write/idle timeouts suitable for long-lived Streamable HTTP and SSE.
- Forward `/.well-known/oauth-protected-resource/<route>` to Labby.
- Forward `/auth/oidc/callback` unchanged when Authelia is selected. Disable
  request-target/query logging on OAuth callbacks: authorization `code`,
  `state`, and provider error parameters must not enter proxy access logs,
  traces, analytics, or error pages.

Labby intentionally uses `Host` for protected-route lookup by default. If a
proxy cannot preserve that authority, `[api].trust_forwarded_headers = true`
makes the first `X-Forwarded-Host` value authoritative for protected-route and
route-metadata selection. This is safe only when the Labby listener is not
directly reachable by clients and every trusted proxy overwrites the inbound
header. It does not enable trust for `X-Forwarded-Proto` or forwarded client-IP
headers. Prefer preserving `Host` and leaving the option at its default `false`.

The built-in authorization, registration, and token rate limits are
process-local, and Labby attributes clients by the direct peer rather than
trusting forwarded client-IP headers. A multi-process deployment therefore
multiplies those limits and must enforce an aggregate limit at the trusted
edge. Never expose Labby's listener directly when trusting forwarded host
authority; the sole trusted proxy must overwrite that header.

## nginx or SWAG

Host-level forwarding is the portable baseline. Both `lab.example.com` and
`mcp.example.com` can point at the same Labby container/listener.

```nginx
server {
    server_name mcp.example.com lab.example.com;

    # OAuth codes and state remain in the upstream request but never enter the
    # edge access log. Both provider callbacks are fixed Labby routes.
    location = /auth/oidc/callback {
        access_log off;
        proxy_pass http://labby:8765;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto $scheme;
    }

    location = /auth/google/callback {
        access_log off;
        proxy_pass http://labby:8765;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto $scheme;
    }

    location / {
        proxy_pass http://labby:8765;
        proxy_http_version 1.1;

        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header Authorization $http_authorization;
        proxy_set_header Accept $http_accept;
        proxy_set_header Content-Type $http_content_type;
        proxy_set_header Mcp-Protocol-Version $http_mcp_protocol_version;
        proxy_set_header Mcp-Method $http_mcp_method;
        proxy_set_header Mcp-Name $http_mcp_name;

        proxy_buffering off;
        proxy_request_buffering off;
        gzip off;
        proxy_read_timeout 1h;
        proxy_send_timeout 1h;
    }
}
```

Use the same upstream for `lab.example.com`; the app host can keep normal
buffering for static assets if your proxy lets you scope MCP streaming behavior
to `mcp.example.com`.

## Caddy

```caddyfile
mcp.example.com {
    reverse_proxy labby:8765 {
        header_up Host {host}
        header_up X-Forwarded-Proto {scheme}
        flush_interval -1
    }
}

lab.example.com {
    reverse_proxy labby:8765 {
        header_up Host {host}
        header_up X-Forwarded-Proto {scheme}
    }
}
```

## Traefik

```yaml
http:
  routers:
    lab-app:
      rule: Host(`lab.example.com`)
      entryPoints: [websecure]
      service: labby
      tls: {}
    lab-mcp:
      rule: Host(`mcp.example.com`)
      entryPoints: [websecure]
      service: labby
      tls: {}
  services:
    labby:
      loadBalancer:
        passHostHeader: true
        servers:
          - url: http://labby:8765
```

Avoid compression and buffering middleware on the MCP router.

## Cloudflare Tunnel

Create public hostnames for the app and MCP gateway that both target Labby:

```yaml
ingress:
  - hostname: lab.example.com
    service: http://labby:8765
  - hostname: mcp.example.com
    service: http://labby:8765
  - service: http_status:404
```

Do not place an Access policy in front of the MCP route unless it is compatible
with MCP OAuth clients. Labby needs to return its own OAuth metadata and bearer
challenge.

## Tailscale Funnel

For a Tailscale-connected Labby host, prefer the setup helper over hand-written
`tailscale funnel` commands:

```bash
# Read-only inspection.
labby setup tailscale-funnel

# Expose the default local listener as public HTTPS.
labby setup tailscale-funnel --apply
```

The default backend is `http://127.0.0.1:8765` on public HTTPS port `443`. Labby
only accepts loopback backends and Tailscale's supported Funnel ports `443`,
`8443`, and `10000`. It refuses to replace a foreign Funnel mapping and refuses
to auto-promote a tailnet-only `tailscale serve` mapping. The mutating configure
and disable actions are local-only administrative operations; inspection remains
read-only.

First-time Funnel activation can require Tailscale web approval. Labby treats
that as an explicit third state rather than success or generic failure: the
configure command is time-bounded, any approval URL emitted by Tailscale is
surfaced, and the operator reruns `labby setup tailscale-funnel --apply` after
approval. Labby does not report the route configured until the desired Funnel
mapping is actually visible.

A successful configuration projects the public origin plus the two exact derived
endpoints needed by the browser integration:

```text
https://<tailnet-dns-name>/auth/google/callback
https://<tailnet-dns-name>/mcp
```

If you put Funnel in front of another local reverse proxy instead, preserve the
external `Host` and all route paths when forwarding to Labby.

## Verification

Run the built-in proxy doctor from any environment that resolves the public
hosts:

```bash
just protected-mcp-smoke -- \
  --app-url https://lab.example.com \
  --mcp-url https://mcp.example.com \
  --route /tools
```

The `just` target wraps `scripts/protected-mcp-smoke`, which delegates to
`labby doctor proxy`. Use `LABBY_BIN=/path/to/labby` or `--labby-bin
/path/to/labby` when testing a specific binary.

The check verifies app health, route-specific protected-resource metadata, and
the expected unauthenticated OAuth bearer challenge on the protected route.
