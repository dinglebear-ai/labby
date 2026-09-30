---
name: install-labby
description: Use when installing, configuring, repairing, or validating Labby, including auth, deployment, public exposure, and agent MCP setup.
---

# Install Labby

Use Labby's CLI/MCP actions as the mutation authority; preserve existing state and prove the live runtime.

## Rules

- Use a verified immutable release, never a mutable branch pipe. Verify installer attestation and SHA-256 before execution.
- Secrets belong in private `LABBY_HOME/.env`; non-secret preferences belong in `config.toml` via `labby setup` / `setup.settings.*`.
- Auth is bearer, OAuth, or both; OAuth selects exactly one inbound provider, Google **or** Authelia. Never weaken auth.
- Codex App Server is an agent/LLM provider, not MCP.
- Existing proxy/service/client config changes require inspection, verified backup, exact proposed change, and explicit approval.

## Flow

1. Inspect host/user/OS/arch/cwd, Labby/state, tools, ports, services, Tailscale, and Incus if relevant.
2. Ask native vs Incus, listener/port, auth/provider, HTTPS/proxy, and clients. Never expose unauthenticated network listeners.
3. Download one release installer + checksum, verify attestation/SHA-256, then run it. Keep secrets out of CLI history.
4. Run `labby setup`; preserve unrelated state and confirm private permissions.
5. Read only the needed branch: [OAuth](references/oauth.md), [Incus](references/incus.md), or [Exposure](references/exposure.md).
6. Use [Verification](references/verification.md); doctor plus a live read-only MCP call is mandatory.
7. With permission, use [Agent setup](references/agents.md), show proposed changes, configure approved clients, reload, and re-test.
8. Report version, deployment/service, URLs, auth topology without secrets, state paths, evidence, and deferred work.

On failure, isolate the failing layer, make the smallest reversible fix, and rerun the same oracle.
