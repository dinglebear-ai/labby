---
title: "Experimental Tailcat browser connection"
created: "2026-10-02"
updated: "2026-10-03"
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
the identity-bound adapter was used. Those results predate the automatic encrypted delivery and native settings flow
described below. Installed-controller setup, the complete encrypted handoff,
Google sign-in and production deployment have not yet been qualified. This remains an experimental integration.

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
- Matched packaged companion assets, or pinned helper/browser assets built with
  `scripts/build-tailcat-bridge.sh`, plus an approved HTTPS DERP map.
- Authenticated Depot with the matching portable browser assets installed.

Microsandbox attaches to Labby as an MCP upstream. It is not attached directly
to ChatGPT or Depot. A connection to the upstream alone does not prove VM
isolation: guest creation must explicitly disable network and omit host mounts.

## Prepare a packaged installation

Release archives built from this checkout include a matched `tailcat/` companion directory next
to the Labby executable. The npm installer retains that directory, and the shell
installer activates and rolls back the executable and companions together.
Source-only builds still require explicitly built assets.

```bash
labby setup --tailcat
```

This preparation verifies the installed assets, then reuses Labby's existing
OAuth setup and persistent keys. It installs no daemon. It keeps the controller
disabled and reports `authorization_required`: authenticated owner bootstrap,
project/loadout/route configuration and local pairing are still required.
`--dry-run` performs inspection without writing configuration. A missing,
wrong-version, wrong-platform or modified bundle stops preparation.

When enabled with no explicit `helper_path`/`helper_sha256` pair, native startup
verifies the sibling bundle manifest and all files before using its helper.
`bundle_path` can select an explicit companion directory. A partial explicit
helper configuration fails rather than falling back. OAuth remains mandatory.

## Configure and enable native pairing

Sign in to the native Labby gateway with its configured administrator account.
Open **Settings → Tailcat** and select a project you directly own.

1. Configure the HTTPS MCP resource, approved HTTPS DERP map and absolute Node
   executable. Labby creates a restricted Microsandbox upstream, loadout and
   route using the verified packaged adapter. The controller stays disabled.
2. Assign the exact displayed loadout using the existing authorized project
   assignment flow, then restart the gateway to publish its policy. This is an
   explicit project access change; setup does not silently replace an assignment.
3. Enroll the project credential. Labby stores it in a private native file and
   displays its path and public credential ID. The browser never receives the
   bearer. Retrying enrollment uses the same request key.
4. Enable pairing, then restart the gateway. Activation rechecks OAuth, direct
   ownership, current credential, exact assignment, published policy and assets.
   Saving the setting does not start a daemon or expose a listener.

The shared actions are `setup.tailcat.configure`, `setup.tailcat.enroll` and
`setup.tailcat.enable`; they require an authenticated native operator session,
CSRF protection and direct project ownership.

The authenticated gateway publishes only a private same-user control socket,
normally `$LABBY_HOME/tailcat/control.sock`. A helper and restricted loopback
MCP listener start after explicit local pairing approval. Missing OAuth, native
authority or verified assets prevents startup.

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

The installer accepts a verified source build or extracted release companion directory.
It checks the corresponding checksum manifest, copies the matching Go runtime and WASM,
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
   The page displays a local command, a pairing code and a fingerprint. Its
   temporary private key stays in browser memory.
3. Run the command on the Labby machine, using the private enrolled credential:

   ```bash
   labby tailcat pair --rendezvous https://depot.example \
     --pairing-id PUBLIC_PAIRING_ID \
     --credential-file /private/path/project-credential
   ```

   Enter the code at the hidden prompt. Compare the fingerprint with the page,
   review the HTTPS origin and upstream, and explicitly approve. For scripted
   approval, supply the code through stdin with `--pair-code-stdin --yes`.
4. The page automatically receives and decrypts the delivery, checks its
   bindings, initializes MCP and lists permitted tools. Depot relays ciphertext;
   the address, preshared key and grant are encrypted to the requesting browser.
5. Exchange codes expire after five minutes and delivery is single-use. Native
   grants expire within 15 minutes. Refreshing or leaving loses the browser key;
   create a new request and approve again.

Codes grant rendezvous access only; native approval and project authorization
remain mandatory. Keep codes out of shell arguments, logs and query strings.
A failed deposit stops the native session; uncertain cleanup requires checking
`labby tailcat status`. A sealed session with no browser activity retires after
its 90-second activity window, including an abandoned handoff.

### Private-file fallback

The existing flow remains available: export a public request, then run
`labby tailcat pair --request /path/request.json --credential-file /private/path/credential --output /private/directory/delivery.json`
and import the new private delivery file into the same page. Keep that delivery
private and remove the owned file after import. Never store it in Git or logs.

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
