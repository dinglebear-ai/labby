---
title: "Experimental Tailcat browser connection"
created: "2026-10-02"
updated: "2026-10-02"
---

# Experimental Tailcat browser connection

Tailcat connects the hosted Depot dashboard to local Labby through an approved
relay. Agents continue to create and work in sandboxes through Labby MCP;
Tailcat is the dashboard connection, not the agent connection. The current
Depot page supports pairing, connection status and tool discovery. VM commands
in the acceptance fixture qualify the transport; they are not dashboard controls. Native Labby still verifies the project credential, its restricted child,
the browser key, origin, session generation and current route policy. Depot is
not an authorization server and does not receive the native bearer or browser
private key.

This integration is under development. An isolated real-Chromium acceptance
fixture has completed browser → Tailcat WASM → relay → native protected Labby
MCP → Microsandbox VM creation, non-root Linux execution and session-owned
cleanup. It inspected disabled networking and empty mounts, denied unrelated VM
cleanup, and verified the guest was absent afterward. The fixture uses local
OAuth configuration. The actual Depot page also passed sign-in, pairing request,
approved delivery import and tool discovery, and the cleanup response confirmed
the identity-bound adapter was used. Final review findings were addressed;
installed-controller setup, Google sign-in and production deployment have not
been qualified. This remains an experimental integration.

## Prerequisites

- macOS or Linux; a Labby build with the `tailcat` feature.
- Hosted Labby with OAuth enabled and its persistent encryption key configured.
- An existing project credential bound to a named, tools-only Loadout containing
  exactly the approved Microsandbox upstream, and an enabled protected MCP route.
  Admin credentials, inline Loadouts and wider projections are refused.
- Microsandbox 0.7.6 with matching native runtime and firmware. For browser
  cleanup, use the experimental adapter in `packages/labby-microsandbox`
  (`npm ci` in that directory, then configure its `server.mjs` as a Node MCP
  upstream). This adapter is not published to npm. Ordinary
  `microsandbox-mcp@0.7.6` remains usable for nondestructive tools, but browser
  deletion is denied because its removal operation targets a reusable name.
  An older runtime in the Microsandbox home directory can override the npm
  package runtime. Set upstream environment variables `MSB_PATH` and
  `MSB_LIBKRUNFW_PATH` to the matching executable and firmware when needed.
- Pinned helper/browser assets built with
  `scripts/build-tailcat-bridge.sh`, plus an approved HTTPS DERP map.
- Authenticated Depot with the matching portable browser assets installed.

Microsandbox attaches to Labby as an MCP upstream. It is not attached directly
to ChatGPT or Depot. A connection to the upstream alone does not prove VM
isolation: guest creation must explicitly disable network and omit host mounts.

## Enable native pairing

Add non-secret transport preferences to the existing Labby configuration:

```toml
[tailcat]
enabled = true
helper_path = "/absolute/path/to/tailcat-bridge"
helper_sha256 = "SHA256_FROM_THE_PINNED_BUILD"
derp_map_url = "https://approved-relay.example/derpmap.json"
```

Start the authenticated hosted gateway normally. This publishes only the private
same-user control socket, normally `$LABBY_HOME/tailcat/control.sock`. A helper
and its restricted loopback MCP listener start only after local approval.
Missing OAuth, native authority or a matching pinned helper prevents startup.

The CLI is local: an explicit remote `--server` or `--context` is refused.
An explicit absolute `--socket` may select another same-host controller; it never
falls back to HTTP.

## Enable the Depot page

Install public assets from the verified build into the intended Depot tree:

```bash
bash scripts/install-tailcat-depot-assets.sh \
  /absolute/build/directory /absolute/depot/directory \
  https://approved-relay.example/derpmap.json
```

The installer checks `SHA256SUMS`, copies the matching Go runtime and WASM,
portable client/hook and notices, and writes a checksum manifest. The four
generated JavaScript modules ship in Depot releases; regenerate them from Labby
rather than editing the copies. WASM, the Go runtime and manifest remain
generated deployment inputs and must be installed for an HTTPS connection.

Configure Depot at runtime:

```text
DEPOT_TAILCAT_ENABLED=true
DEPOT_TAILCAT_ORIGIN=https://depot.example
DEPOT_TAILCAT_CONNECT_ORIGINS=https://approved-relay.example,wss://selected-derp.example
```

List every approved map/DERP connection origin explicitly. The sandbox page alone
permits WASM compilation and these connections. Public-mode anonymous readers
cannot open it. Leave the feature disabled until the deployment is qualified.

## Pair and connect

1. Sign in to Depot and open `/ui/sandboxes` over HTTPS.
2. Enter the configured Microsandbox upstream name and create a pairing request.
   The browser downloads public request JSON; its private key stays in memory.
3. On the Labby machine, keep the existing project credential in a private file
   owned by the current user. Approve the request:

   ```bash
   labby tailcat pair --request /path/to/labby-pairing.json \
     --credential-file /private/path/project-credential \
     --output /private/directory/labby-delivery.json
   ```

   Review the displayed HTTPS origin, upstream and key/nonce fingerprint.
   Noninteractive use requires explicit `--yes`. The output directory must
   already be private; the delivery is a new private file and is never overwritten.
4. Import that delivery file into the same browser page. Depot confirms only
   public metadata against its pending authenticated session. The browser uses
   its memory-held key and sealed grant to initialize MCP and list permitted tools.
5. The native grant expires within 15 minutes. Refreshing/navigating away loses
   the browser key; create a fresh request and approve again.

The delivery file is a sensitive capability. Keep it private and remove the
owned file after importing it. Neither secret belongs in command arguments,
logs, HTML attributes, query strings or Git.

## Sandbox cleanup scope

VMs created through this session must use a unique name of the form
`labby-tailcat-<32 lowercase hexadecimal characters>`. Labby adds its own
ownership label and records successful creation in native memory. The session
can remove only those VMs. The adapter checks the current ownership label and
uses the SDK identity-bound handle destruction, which refuses a same-name
replacement. Labby requires the adapter's atomic cleanup capability before
authorizing deletion. Creation refuses replacement options. The session
cannot remove an existing VM or one created through another session. Other
destructive tools keep their existing project access policy.

Each session can track up to eight sandboxes, including uncertain creation or
cleanup attempts. A failed inspection releases its reservation for another
explicit attempt. Once deletion is dispatched, an uncertain outcome is not
replayed automatically.
The browser sends a ping every 30 seconds. After the first successful request,
90 seconds without successful browser activity retires the native grant, helper
and listener. An unused delivery still expires at its original deadline.
Closing or expiring a session does not delete its VMs; use native Microsandbox
recovery if cleanup was not confirmed. VM lifetime limits should remain enabled.

## Stop and recover

```bash
labby tailcat status
labby tailcat stop SESSION_ID
```

Browser disconnect closes browser streams. Native stop retires the child
credential, listener and helper. Revoking the original project credential also
ends the session. Closing the hosted controller retires its owned sessions.

A lost approval response can mean a session started. Inspect `status` before
trying again; the CLI does not replay an uncertain approval. If delivery-file
publication fails, it attempts one native stop and reports unconfirmed cleanup.
Failed connection or expired pairing requires a new browser key and approval.
