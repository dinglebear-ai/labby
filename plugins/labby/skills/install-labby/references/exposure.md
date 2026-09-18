# HTTPS exposure and reverse-proxy reference

## Tailscale Funnel

Search the current official Tailscale Funnel documentation before setup:
https://tailscale.com/docs/reference/tailscale-cli/funnel

As of the current validated docs, tailscale funnel --bg persists the Funnel configuration and automatically resumes after reboot or a Tailscale down/up cycle. Do not add a duplicate systemd/launchd wrapper solely for Funnel persistence.

Important current constraint: Funnel's HTTP reverse-proxy target is a loopback service. Verify the current docs before assuming it can target an arbitrary Tailnet/LAN address.

Typical shape:

~~~bash
tailscale status
curl -fsS http://127.0.0.1:PORT/ready
tailscale funnel --bg http://127.0.0.1:PORT
tailscale funnel status
~~~

Use the current supported HTTPS listener ports from Tailscale's docs. Do not assume arbitrary external ports are accepted.

If the Labby process is bound only to a Tailnet-specific address and Funnel cannot target it, do not improvise an insecure bridge. Ask whether the operator wants to:
- rebind Labby to loopback or an address that still accepts loopback traffic
- use a supported local reverse proxy target
- skip Funnel

Once the public Funnel URL is known, ensure LABBY_PUBLIC_URL matches the public browser origin before OAuth configuration.

## Existing reverse proxy

A reverse proxy is an approval-gated branch.

First ask for:
- product, such as Caddy, nginx, Traefik, SWAG, HAProxy, Nginx Proxy Manager, Cloudflare Tunnel, or another proxy
- host/device
- absolute configuration path
- intended public hostname

### Before touching config

1. Search the web for the latest official documentation for the exact proxy/version when possible.
2. Inspect the running configuration and validation/reload mechanism.
3. Identify every file that would be modified.
4. Create a backup directory outside the live config tree when practical, with a timestamp and restrictive permissions.
5. Copy every target file preserving metadata.
6. Compute a cryptographic digest for original and backup copies.
7. Verify the backup bytes match the originals.
8. Record the backup directory and digest manifest.
9. Prepare the exact proposed diff without applying it.
10. Tell the user:
   - backup location
   - files backed up
   - integrity verification result
   - exact files proposed for modification
   - exact routing/TLS/header changes
   - reload/restart command that would be used
   - rollback command/path

Then ask for explicit approval to proceed.

Do not interpret "yes, I use nginx" as approval to edit nginx.

### After approval

Apply the smallest change possible.

Preserve unrelated operator config.

Validate syntax before reload. Examples only, not universal commands:
- nginx -t
- caddy validate
- traefik config-specific validation
- product-provided dry run

Use the exact supported validator for the deployed product.

Reload rather than restart when the product supports a safe reload and the change does not require a restart.

Verify from both sides:
- proxy can reach Labby
- user can reach the public domain
- HTTPS certificate is valid
- /ready or an equivalent safe endpoint behaves as expected
- /mcp reaches the MCP transport
- OAuth metadata/callback endpoints match the public origin when OAuth is enabled
- no unexpected redirect loop, auth stripping, websocket/SSE breakage, or path rewriting exists

Show the user the domain and ask them to confirm successful access.

Do not proceed to final onboarding until the user confirms the domain is working.

### Debug loop

If the user reports an error:
1. capture the exact browser/proxy/status error
2. inspect current proxy and Labby logs
3. search current official docs for that product/error
4. compare required proxy headers/path/streaming behavior
5. inspect DNS and TLS separately from upstream routing
6. test direct Labby access
7. test proxy-to-Labby access
8. test public access
9. fix one layer at a time
10. rerun the failed check

Never disable Labby authentication or TLS verification as a debugging shortcut.
