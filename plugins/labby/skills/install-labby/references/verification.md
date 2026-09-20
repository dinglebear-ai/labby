# Verification

Require `labby doctor`, health/readiness, persistence, a live MCP discovery plus one read-only `gateway.help` call, Code Mode status, and clean new logs. Config files alone are not proof.

For bearer-authenticated HTTP, resolve `scripts/verify-labby-mcp.sh` relative to this Skill's `SKILL.md`, regardless of the caller's current working directory. Supply the token through the environment, never an argument or shell history:

```bash
skill_dir=/absolute/path/to/install-labby
LABBY_MCP_TOKEN="$token" \
LABBY_EXPECTED_VERSION="$installed_release" \
  bash "$skill_dir/scripts/verify-labby-mcp.sh" "https://labby.example/mcp"
```

The verifier directly exercises Labby's stateless HTTP MCP boundary, requires the `gateway` tool, and calls `gateway` with `action: help`. It defaults to MCP protocol `2025-06-18`; set `LABBY_MCP_PROTOCOL_VERSION` only when the installed Labby release and client contract require another explicit version.

Requests are bounded to a 10-second connection timeout and a 30-second total timeout. Override those bounds only with positive integer seconds in `LABBY_MCP_CONNECT_TIMEOUT` and `LABBY_MCP_MAX_TIME` when the deployment's measured latency requires it.

For OAuth-only deployments, use the approved OAuth-capable client configured in [Agent setup](agents.md), record that client's installed version, and perform the same `tools/list` plus read-only `gateway.help` call. Do not install or execute an unpinned package runner merely to verify MCP.

Report the Labby version, protocol version, tool count, successful help call, URLs/auth topology without secrets, persistence result, Code Mode status, and any new warning/error logs.
