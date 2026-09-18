# Deployment verification reference

A successful setup requires runtime proof, not just correct-looking files.

## 1. Labby health

Run:
- labby --version
- labby doctor
- labby health when available
- service/container status
- a loopback readiness request

Inspect Labby logs if doctor reports any degraded capability.

## 2. Secret-file verification

Check without printing values:
- LABBY_HOME directory is private
- .env exists
- .env is not a symlink
- .env is owned by the intended runtime account
- Unix mode is 0600
- config.toml contains non-secret preferences only
- OAuth secrets and bearer tokens are absent from config.toml

For OAuth-only, LABBY_MCP_HTTP_TOKEN should be empty/absent according to the current setup contract.
For OAuth plus bearer, it should be present but never printed.

## 3. Service persistence

Native Linux:
- systemctl is-enabled labby
- systemctl is-active labby
- inspect the actual unit and any drop-ins

Native macOS:
- inspect the Labby LaunchAgent
- verify it is loaded and points at the installed binary/config

Incus:
- incus config get CONTAINER boot.autostart
- systemctl status labby inside the container
- curl the internal readiness endpoint

## 4. Code Mode

Use:
- labby gateway code status
- enable it through the supported command if the user accepts it
- verify status again

This is the preferred way to materialize the first non-secret gateway preference in config.toml rather than hand-writing a new file.

## 5. MCPorter live smoke

Use the current mcporter CLI. Current upstream docs support:

~~~bash
npx mcporter --version
npx mcporter list
npx mcporter list SERVER --schema --json
npx mcporter call SERVER.TOOL --args '{"action":"help"}' --output text
~~~

Do not assume SERVER is named lab or TOOL is named setup until discovery confirms it.

For a one-off endpoint, consult npx mcporter --help/current docs and configure the Streamable HTTP URL using:
https://PUBLIC_OR_LOCAL_LABBY/mcp

Auth handling:
- bearer mode: use a protected environment-backed token mechanism supported by current mcporter
- OAuth mode: use mcporter auth when appropriate
- never put a bearer token directly into shell history if an environment/vault mechanism exists

The read-only oracle should call a Labby service help action. setup.help or gateway.help are appropriate because they do not mutate state.

Pass criteria:
- initialize succeeds
- server is healthy
- target tool is listed
- help action returns a real Labby catalog response
- no MCP protocol/auth error appears

## 6. Agent-side smoke

After configuring an agent and restarting/reloading it:
1. ask the agent to discover Labby
2. ask it to invoke a read-only Labby help action
3. verify the request appears in Labby usage/log telemetry when available
4. confirm the response is from the expected deployment

If mcporter succeeds but the agent fails, the problem is client configuration or client auth, not the Labby service. Debug that layer instead of reinstalling Labby.

## 7. Final evidence

Record:
- Labby version
- deployment type
- host/container
- service state
- listener/public URLs
- auth topology, without credentials
- doctor result
- MCPorter command/result
- agent smoke result
- Code Mode state
- any warnings/deferred work

Never report completion if a required check was skipped or failed.
