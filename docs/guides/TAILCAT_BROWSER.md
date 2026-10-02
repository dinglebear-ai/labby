# Experimental Tailcat browser connection

Tailcat carries MCP requests from a browser to local Labby through an approved
relay. Native Labby still verifies the project credential, its restricted child,
the browser key, origin, session generation and current route policy. Depot is
not an authorization server and does not receive the native bearer or browser
private key.

This integration is under development. Its configuration and process lifecycle,
browser transport package, and Go/WASM bridge have focused regression coverage.
That coverage does not qualify an installed native controller, the Depot
integration, or a successful browser → Labby → network-disabled Microsandbox VM
run. Live acceptance and final review remain required before publication.

## Prerequisites

- macOS or Linux; a Labby build with the `tailcat` feature.
- Hosted Labby with OAuth enabled and its persistent encryption key configured.
- An existing project credential bound to a named, tools-only Loadout containing
  exactly the approved Microsandbox upstream, and an enabled protected MCP route.
  Admin credentials, inline Loadouts and wider projections are refused.
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
portable client/hook and notices, and writes a checksum manifest. Generated
assets are not source files; include them when packaging the Depot deployment.

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
