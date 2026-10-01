---
name: install-labby
description: Use when installing, configuring, repairing, or validating Labby, including auth, deployment, public exposure, and agent MCP setup.
---

# Install Labby

Use Labby's CLI/MCP actions as the mutation authority; preserve existing state and prove the live runtime.
This skill is optional guidance for installation, advanced deployments, and repair.
The standalone installer and binary-owned first-use flow are the recommended product path.

## Rules

- The recommended canonical HTTPS bootstrap explicitly trusts the initial installer/helper source and reviewed verifier hashes; it verifies immutable release payload provenance before activation. For independent installer verification, verify the tagged installer's attestation and SHA-256 before execution. Never substitute an arbitrary mutable branch pipe.
- Use protected setup/settings writers for secrets and preferences. The recommended path requires no manual `.env`, TOML, or client JSON edits.
- Auth is bearer, OAuth, or both; OAuth selects exactly one inbound provider, Google **or** Authelia. Never weaken auth.
- Codex App Server is an agent/LLM provider, not MCP.
- Existing proxy/service/client config changes require inspection, verified backup, exact proposed change, and explicit approval.

## Flow

1. Inspect host/user/OS/arch/cwd, Labby/state, tools, ports, services, Tailscale, and Incus if relevant.
2. Establish whether the user wants Labby on this computer or an existing gateway. Use native loopback/bearer defaults for local first use; infrastructure choices belong in explicitly requested advanced setup. Never expose unauthenticated network listeners.
3. Use the canonical HTTPS standalone installer for the recommended route, retaining its explicit initial-script trust boundary. For independent verification, download one immutable release installer, checksum, and public attestation bundle and verify them with an already trusted verifier before execution. The reviewed pinned verifier bootstrap trusts fixed archive hashes in its trusted source, not an unchecked downloaded checksum. Public bundles require no developer account; older releases without bundles require authenticated lookup. Keep secrets out of CLI history.
4. Run `labby setup`; preserve unrelated state, confirm private permissions, and use the short-lived local browser handoff where supported. Installation success does not mean onboarding is complete.
5. Read only the needed branch: [OAuth](references/oauth.md), [Incus](references/incus.md), or [Exposure](references/exposure.md).
6. In Agent provider settings, select the supported protocol, save the protected connection, discover actual models, and create a personal starter Agent. Require a bounded completed Agent run; listing models or saving credentials is insufficient. `openai` uses standard APIs; `phoenix` requires session extensions and remains the legacy default when omitted. Addresses must be reachable from the gateway host.
7. Distinguish built-in Agents from external applications. With authorization, use [Agent setup](references/agents.md), inspect proposed changes/backups, register selected supported clients through the local helper, and verify gateway-observed client tool use. Registration or an OAuth login requirement is not a verified connection. A remote browser cannot edit laptop applications.
8. Qualify Discover under its configured access policy. Add to Library saves an artifact; Add MCP server requires supported revision metadata, explicit endpoint/access approval, credentials, runtime checks, and a reviewed safe tool call. Use [Verification](references/verification.md); doctor alone is insufficient.
9. Read authenticated `setup.readiness.state`. Report version, deployment/service, URLs, auth topology without secrets, state paths, each required check's evidence, and deferred work. Failed or skipped required provider/Agent/catalog/MCP checks never count as fully ready.

On failure, isolate the failing layer, make the smallest reversible fix, and rerun the same oracle.
