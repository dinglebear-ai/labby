# Agent setup

## Built-in Agents

Connect the gateway-host-reachable provider in Settings, select `openai` or `phoenix`, discover real model IDs, and create a personal starter Agent. Verify a bounded completed run. Saving provider credentials, a model list, or an Agent definition alone is not readiness.

## External applications

Use existing user authorization to inspect and configure selected supported clients. `labby setup clients list --json` reports Codex and Claude Code detection. Review `labby setup clients plan --client <client> --gateway-url <url>` and its conflict/version information; registration uses that exact `--expected-version`. The recommended saved-connection helper is `labby setup clients connect --clients <client>`, which verifies the saved target, preserves other MCP entries, and creates protected backups. Consult its current `--help` before adding deployment-specific flags.

Bearer registration uses the protected local Labby bridge. Application configuration contains the executable, state-root, and gateway target, not bearer secrets. Gateway-issued client observation sessions supplement real authentication and grant no authority. Reload the application and explicitly approve a safe first tool call; require gateway-observed successful use for each selected client. A registered entry, `mcp list`, or a direct helper call cannot prove the external application used Labby.

OAuth Connect uses `--connection oauth` (`o-auth` remains a compatibility alias). First run `labby auth login --server https://gateway.example` against the exact selected HTTPS server origin, using the same selected installation root as Connect. This supported flow requires `/v1/setup` and `/mcp` on that same origin; mounted base paths and separate UI/control-plane and MCP origins are unsupported. Connect issues per-client observation proofs through the authenticated control plane and writes only the supplemental observation header into protected client configuration. Each application must complete its own OAuth login as that same user and perform a successful tool call before its use is verified. Observation proofs expire after one hour; rerun Connect to renew them. Expiry ends observation without revoking an otherwise valid OAuth login or refreshing historical readiness evidence. Plan/Register alone do not issue observation sessions. Never add a static bearer or OAuth access-token header to an OAuth registration. A remote gateway browser cannot write client files on the user's laptop; the local helper must run on that laptop.
