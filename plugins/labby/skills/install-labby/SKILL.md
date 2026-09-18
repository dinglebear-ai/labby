---
name: install-labby
description: "Use when installing Labby for the first time or converting an existing binary-only install into a fully configured Labby deployment. Guides verified release installation, server authentication, listener scope, Incus or native deployment, Tailscale Funnel, reverse-proxy handoff, agent MCP configuration, doctor and mcporter smoke tests, and optional Phoenix Codex App Server assistance."
---

# Install Labby

Use this skill as Labby's first-class guided installation path.

The skill orchestrates the operator experience. The Labby binary remains the authority for durable setup, credential writes, service installation, repair, gateway mutation, and validation. Prefer current Labby CLI/MCP actions over reproducing their behavior in shell or editing Labby-owned state by hand.

When repository docs and this skill disagree, inspect current generated CLI/action catalogs and current official third-party documentation before proceeding.

## Non-negotiable rules

1. Install a verified immutable release. Do not pipe a branch or raw GitHub copy directly into a shell. Download the release installer and checksum for one explicit release, verify GitHub attestation plus SHA-256, then execute the installer.
2. Keep secrets in LABBY_HOME/.env. Keep non-secret product preferences in LABBY_HOME/config.toml. Never place OAuth client secrets, bearer tokens, access tokens, or provider API keys in config.toml, chat output, shell history, source control, or issue text.
3. Let labby setup and setup.settings.* perform Labby-owned state mutation whenever they support the operation. The environment merge path is atomic, backup-backed, and owner-only.
4. Never claim Google and Authelia can both be active inbound OAuth providers on one Labby instance. Labby currently supports exactly one inbound OAuth provider at a time: google or authelia.
5. Labby supports bearer-only, OAuth-only, and OAuth plus static bearer break-glass. Existing --oauth google or --oauth authelia calls without --auth preserve the historical OAuth plus bearer behavior.
6. Codex App Server is not an MCP server. Use it for Phoenix/agent-harness assistance. Use Codex's MCP client support when configuring Codex to consume Labby.
7. Do not assume browser control exists. Before an OAuth-console or Web UI handoff, discover available browser/computer capabilities. Prefer an already connected Chrome DevTools/browser MCP, Computer Use, Claude-with-Chrome/browser tooling, or ChatGPT/Work cloud browser when that runtime exposes one. Use the actual available tool rather than inventing a browser capability. If none exists, print the exact URL and exact steps.
8. Do not edit a reverse proxy configuration until the user explicitly approves the exact proposed files and changes after backups have been created and verified.
9. Never weaken authentication to make a failing deployment pass.
10. Do not proceed past a verification gate that the user explicitly must confirm, such as a reverse-proxied domain that only the user can validate from their environment.

## Phase 1: Inspect the machine and current Labby state

Before changing anything:

- Confirm hostname, current user, OS, architecture, home directory, and current working directory.
- Check whether labby, curl, gh, a SHA-256 utility, Node/npx, and Tailscale are available.
- If the operator is considering Incus, check whether the host is Linux x86_64, whether Incus is installed and initialized, and whether the daemon is reachable.
- Check for an existing LABBY_HOME, normally ~/.labby, existing .env/config.toml, existing Labby services, and a current labby version.
- If existing Labby state is present, preserve it. Do not blindly replace config or credentials.
- Inspect current labby setup --help, labby doctor --help, labby gateway --help, and the generated action catalog when running from a checkout.

Tell the user what was found before any destructive or security-sensitive migration.

## Phase 2: Collect the deployment choices

Ask for the choices below. Group related questions, but do not bury important security choices in one giant prompt.

### Authentication

Ask:

1. Bearer token only
2. OAuth only
3. OAuth plus bearer break-glass

If OAuth is selected, ask for exactly one provider:

- Google
- Authelia

If the user asks for Google and Authelia simultaneously, explain that the current runtime selects one inbound provider per instance. Do not fabricate multi-provider support.

If OAuth is selected, ask for the email address that should bootstrap the Labby admin identity.

For bearer-only or both, let Labby generate the secure bearer token. Do not invent a shorter token. The canonical setup path generates a 64-character random token and stores it in the protected .env file.

### Listener

Ask where Labby should listen:

- Loopback: 127.0.0.1
- Tailnet: the host's current Tailscale IPv4 address
- LAN: preferably the intended interface address; use 0.0.0.0 only when the user explicitly wants all interfaces

Ask for a port. If omitted, select a random high port in 49152 through 65535, reject known conflicts, verify it is currently bindable, and verify again immediately before service activation. Remember that any free-port test has a race; the real service bind is the final oracle.

Never expose an unauthenticated network listener.

### Deployment boundary

Ask whether the user wants Labby inside the supported isolated Incus system container.

Explain the current facts:

- Incus is the recommended self-hosted Labby gateway boundary.
- The supported prebuilt release path is Linux x86_64 with an Ubuntu 26.04 system container.
- The container is unprivileged and is intended to isolate Labby-launched stdio MCP servers from the host.
- The release image includes the Labby binary plus Node, Python/uv, Rust, Go, Claude Code, Codex, Gemini CLI, Tailscale, and other operator tooling.
- Incus installation/initialization on the host is not silently performed by Labby.

If the host cannot support this deployment, say so and offer native deployment or a remote compatible Incus host.

If the user chooses Incus, ask whether they also want an agent sandbox inside the Labby container.

For Codex sandbox assistance:
- configure/login Codex inside the labby account
- use Phoenix's codex_app_server provider for Labby's assistant surface
- do not register codex app-server as an MCP upstream

For Claude:
- install/login is already part of the supported runtime floor
- before wiring Claude as an agent harness or MCP-like upstream, inspect current Claude Code capabilities and official documentation
- do not invent a Claude MCP server mode that is not present

Read references/incus.md before this branch.

### Public HTTPS

Ask whether to use Tailscale Funnel for public HTTPS access.

If yes, read references/exposure.md. Funnel --bg is the persistence mechanism; do not build a second reboot service around it.

Ask whether the user also has an existing reverse proxy they want configured. If yes, ask for:
- reverse proxy product
- device/host where it runs
- absolute path to its configuration

Then follow the approval-gated reverse-proxy procedure in references/exposure.md.

## Phase 3: Install the verified release

Prefer the latest stable release unless the user requests a specific version.

Do not execute an unverified remote script.

Canonical release flow:

~~~bash
version="vX.Y.Z"
base="https://github.com/dinglebear-ai/labby/releases/download/$version"

curl -fSLO "$base/labby-install.sh"
curl -fSLO "$base/labby-install.sh.sha256"

gh attestation verify labby-install.sh \
  --repo dinglebear-ai/labby \
  --signer-workflow dinglebear-ai/labby/.github/workflows/release.yml \
  --source-ref "refs/tags/$version" \
  --deny-self-hosted-runners

shasum -a 256 -c labby-install.sh.sha256
~~~

On Linux, sha256sum is also acceptable.

The installer performs its own verified binary release installation. To make the agent-guided setup deterministic, either let it enter interactive setup or pass resolved LABBY_SETUP_* values. Supported handoff includes:

- LABBY_SETUP_ROLE=server
- LABBY_SETUP_DEPLOYMENT=native|incus
- LABBY_SETUP_HOST
- LABBY_SETUP_PORT
- LABBY_SETUP_PUBLIC_URL
- LABBY_SETUP_AUTH=bearer|oauth|both
- LABBY_SETUP_OAUTH=none|google|authelia
- LABBY_SETUP_DESKTOP
- LABBY_SETUP_NO_BROWSER

Provider credentials remain ordinary protected environment variables, not CLI arguments.

When credentials must be supplied non-interactively, avoid putting secrets directly in shell history. Prefer an already-protected environment source or interactive secret prompt.

## Phase 4: OAuth provider setup

If Google is selected, read references/oauth.md and use the exact Google callback URL:

https://PUBLIC_LABBY_ORIGIN/auth/google/callback

If Authelia is selected, use:

https://PUBLIC_LABBY_ORIGIN/auth/oidc/callback

OAuth requires a public URL that matches the browser-visible origin. Remote plain HTTP is not acceptable.

For Google, try to open the current Google Auth Platform Clients page with an available browser/computer tool. If no browser tool is available, provide the URL and exact instructions instead.

Do not continue until the required client ID/secret/admin email are present in the protected setup path.

## Phase 5: Finish Labby-owned configuration

Run the resolved labby setup flow.

After setup:

- confirm LABBY_HOME/.env exists, is not a symlink, is owned by the expected account, and is mode 0600 on Unix
- confirm LABBY_HOME itself is private
- do not print secret values
- inspect the setup settings schema/state before touching config.toml
- if config.toml does not yet exist, establish it through the canonical stale-protected `setup.settings.config.update` path with a non-secret preference chosen by the user; a harmless default such as the current `output.format` is acceptable when the user has no preference. Read the current value first and send it back as `previous`; never bypass stale-write protection
- if Code Mode is desired, enable it through the supported Labby gateway Code Mode action/CLI; this is also a valid first non-secret config.toml preference
- preserve every unrelated existing TOML section and setting
- verify config.toml parses through setup check/doctor and contains no secrets
- run labby gateway code status and verify it matches the intended state

If config.toml already exists, preserve unrelated settings.

## Phase 6: Persistence

Native deployment must survive reboot.

- Linux: use Labby's supported hardened systemd host-service path.
- macOS: use Labby's supported LaunchAgent installer path.
- Incus: verify the container has boot.autostart=true and that labby.service is enabled and healthy inside the container.

Do not substitute ad hoc nohup, tmux, screen, or a custom cron job for the supported service path.

## Phase 7: Tailscale and reverse proxy

Follow references/exposure.md.

For Funnel, verify:
- Tailscale is connected
- the Labby local target is reachable from loopback
- tailscale funnel --bg is configured
- tailscale funnel status reports the intended public URL
- the Labby public URL is updated consistently for OAuth/callback generation
- the endpoint survives a Tailscale restart or reboot semantics according to current official docs

For a reverse proxy, stop at the explicit approval gate before edits.

## Phase 8: Doctor and live MCP smoke

Read references/verification.md.

At minimum:
1. labby doctor
2. readiness/health check
3. service persistence check
4. npx mcporter list against the live Labby MCP endpoint
5. one read-only service help action through the live MCP transport
6. Code Mode status
7. inspect logs for new errors

Do not call the deployment successful from config files alone.

## Phase 9: Agent discovery and client configuration

Ask permission before scanning user agent configs.

If approved:
- run `labby gateway discover --json` first because it is read-only
- explain that this scans MCP servers already configured in supported client config files so Labby can optionally import those upstreams
- use the returned `source_client` and config paths as evidence of configured clients, then supplement that with read-only executable/config probes for supported clients that have no MCP servers yet
- supported discovery families currently include Cursor, Claude Code, Claude Desktop, Codex, Windsurf, OpenCode, VS Code, and Gemini
- report which clients are actually present before proposing changes
- do not confuse gateway discover/import with registering Labby itself into those clients
- show proposed imports before `labby gateway import` and obtain confirmation before bulk import

Then read `references/agents.md`. Identify installed MCP-capable clients using both the read-only Labby config scan and ordinary host inspection of known client executables/config locations; `gateway discover` only proves that MCP configuration was found, not that every installed agent was detected. Show the detected-client list to the user, then configure Labby into each approved client using that client's current supported command/config surface.

Important Claude plugin caveat: the checked-in `.mcp.json` always emits a static `Authorization: Bearer ${user_config.api_token}` header, so treat it as a bearer convenience only. Current Claude Code does not fall back to OAuth after an explicitly configured Authorization header is rejected. For OAuth-oriented Claude use, register the Labby HTTP MCP endpoint through a higher-precedence client scope without the static header, then use Claude's supported OAuth login flow.

For Codex:
- verify current official OpenAI MCP docs and codex mcp --help before mutation
- prefer codex mcp add with the Labby Streamable HTTP URL when supported
- for bearer auth, prefer a bearer-token environment variable reference instead of embedding the token
- for OAuth, use Codex's supported MCP OAuth login flow
- verify with codex mcp list

For Claude Code:
- inspect current official Claude Code MCP docs and `claude mcp --help`
- for bearer use, the plugin MCP entry or a native bearer registration is acceptable
- for OAuth use, do not rely on the plugin's static bearer-header entry; add the endpoint without a static Authorization header in a higher-precedence client scope and run the current supported OAuth login flow
- for OAuth + bearer, ask whether this client should use OAuth identity or the static break-glass credential
- prefer the supported CLI over editing JSON by hand
- never embed the Labby bearer token in a tracked project file

For every other detected client, inspect that client's current official documentation before editing its config.

Tell the user exactly which files/CLI registrations will change before changing them.

## Phase 10: Restart clients and verify from the agent

Instruct the user to restart/reload each configured agent where required.

Then have the agent perform a read-only live smoke through Labby:
- discover the Labby server/tool
- call the setup or gateway service with action=help
- confirm the response came from the deployed Labby instance

If that fails, debug systematically using the exact transport/auth error, labby doctor, server logs, client logs, current official docs, and a direct mcporter comparison.

Do not stop at "the config looks right."

## Phase 11: Optional LLM assistance

Ask whether the user wants Phoenix LLM assistance.

Codex App Server is the default first-class local provider. If selected:
- verify codex is installed in the Labby runtime
- authenticate Codex as the Labby runtime user
- inspect current Labby settings schema
- configure the phoenix provider through supported settings mutation when a change is required
- keep provider secrets in .env
- smoke a Phoenix session only after Codex login is valid

For an OpenAI-compatible provider, collect its base URL and optional API key, then use LABBY_PHOENIX_OPENAI_BASE_URL and LABBY_PHOENIX_OPENAI_API_KEY in .env. Verify the provider actually exposes the session lifecycle Labby expects.

## Phase 12: Handoff

At completion, tell the user:

- deployed Labby version
- deployment shape and service/container name
- listener URL
- public Web UI URL, if any
- MCP URL
- authentication mode and OAuth provider, without printing secrets
- where LABBY_HOME/.env lives
- where LABBY_HOME/config.toml lives
- where the bearer token can be retrieved by the operator, if one exists
- where verified installer receipts/backups live
- doctor result
- mcporter live-smoke result
- which agents were configured and which need restart/reload
- any deferred reverse-proxy or OAuth work

If browser control is available, open the deployed Web UI. If it is not, print the exact browser URL prominently.

When a static bearer exists, tell the operator that its authoritative value is `LABBY_MCP_HTTP_TOKEN` in the protected server `.env`; do not print the value. For OAuth-only installs, explicitly say there is no static bearer credential.

Then ask whether they have any further questions.

Thank the user and their agent for checking out Labby, and point issues/requests to:
https://github.com/dinglebear-ai/labby/issues

Invite them to:
- open Discover and browse/search the catalog of Skills, MCP servers, subagents, plugins, marketplaces, commands, prompts, hooks, and other artifacts, using Code Mode to keep large tool catalogs from bloating the model context
- open Create to build artifacts by hand or ask Phoenix to help forge one
- create a Team and invite collaborators through Labby's Team invitation/access surfaces, then share useful artifacts in that Team
- build a devcontainer for an isolated project environment

Do not hard-code a catalog item count in the success message unless the deployed Labby instance reports the current count. Marketing numbers age faster than installers.

## Troubleshooting rule

For any user-reported failure:
1. capture the exact error and current state
2. classify the failing layer: installer, setup, service, auth, network, proxy, MCP transport, client config, or agent
3. inspect Labby logs/doctor output
4. search current official documentation for the failing third-party component
5. form one root-cause hypothesis at a time
6. apply the smallest reversible fix
7. rerun the same failing oracle
8. only proceed after the current gate passes

Never hide degraded functionality behind a successful-looking final message.
