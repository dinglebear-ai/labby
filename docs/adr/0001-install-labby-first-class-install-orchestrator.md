---
title: "ADR 0001: Make install-labby the First-Class Guided Installation Orchestrator"
created: "2026-09-18"
updated: "2026-09-18"
---

# ADR 0001: Make install-labby the First-Class Guided Installation Orchestrator

Date: 2026-09-18

Status: Accepted

## Context

Labby already has strong binary-owned installation and setup primitives:

- a verified release installer with GitHub attestation, SHA-256 verification,
  activation journaling, receipts, and offline rollback;
- `labby setup` for server/client onboarding, native and Incus deployment,
  provider credentials, persistence, and browser handoff;
- schema-driven setup/settings actions;
- gateway discovery/import, Code Mode, doctor, and logs;
- an Incus gateway image with agent/toolchain runtimes;
- a Phoenix assistant backed by Codex App Server.

What was missing was one operator experience that could discover the machine,
ask the relevant questions, explain tradeoffs, drive those primitives in the
right order, verify the running system, configure the user's agents, and
systematically debug failures.

Embedding that orchestration directly in the shell installer would make the
installer interactive, difficult to keep current with third-party provider
consoles, and responsible for browser/computer workflows it cannot safely own.
Conversely, teaching an Agent Skill to write `.env`, `config.toml`, service
units, or gateway state directly would duplicate security-sensitive behavior
already implemented and tested in Rust.

The checked-in plugin also contained stale installation guidance and a
deployment-specific default, making it unsuitable as a clean public first-run
surface.

## Decision

`plugins/labby/skills/install-labby` is Labby's first-class **guided**
installation orchestrator.

Users may install only that skill with:

```bash
npx skills add https://github.com/dinglebear-ai/labby --skill install-labby
```

and invoke it in a skill-aware agent as `$install-labby`.

The skill owns conversation and orchestration:

- machine and existing-state inspection;
- authentication, listener, deployment, isolation, and exposure questions;
- current third-party documentation lookup;
- browser/computer guidance where available;
- reverse-proxy backup/proposal/approval sequencing;
- invoking the verified release installer and Labby-owned setup actions;
- agent discovery/configuration guidance;
- doctor, service, MCPorter, and agent-side runtime verification;
- systematic debugging and final operator handoff.

The Labby binary remains the mutation authority:

- the release installer owns release trust and activation;
- `labby setup` owns first-run durable configuration;
- `setup.settings.*` owns supported settings mutation;
- gateway commands/actions own gateway and Code Mode state;
- service installers own persistence;
- doctor/log surfaces own runtime diagnosis.

The skill must prefer those interfaces over hand-editing Labby-owned state.

## Authentication contract

Setup exposes authentication topology independently from provider selection:

```text
--auth bearer|oauth|both
--oauth none|google|authelia
```

- `bearer`: generated static bearer only.
- `oauth`: selected OAuth provider with no static bearer.
- `both`: selected OAuth provider plus generated static bearer break-glass.

For backwards compatibility, an existing noninteractive invocation that passes
`--oauth google` or `--oauth authelia` without `--auth` resolves to
`both`, preserving the historical break-glass behavior.

Labby's lower-level runtime continues to use
`LABBY_AUTH_MODE=bearer|oauth`. An OAuth + bearer deployment therefore has
`LABBY_AUTH_MODE=oauth` plus a non-empty `LABBY_MCP_HTTP_TOKEN`.

One Labby instance selects exactly one inbound human OAuth provider. Google and
Authelia are alternatives, not simultaneously active providers.

Secrets remain in `LABBY_HOME/.env`. Non-secret product preferences remain in
`LABBY_HOME/config.toml`.

## Deployment and exposure contract

Incus remains the recommended self-hosted gateway boundary where the supported
host requirements are met. Native service deployment remains supported.

Tailscale Funnel configuration follows the current official Tailscale behavior.
The skill must re-check upstream documentation before use rather than encode a
parallel persistence service.

Reverse-proxy work is explicitly approval-gated. Before any edit, the agent
must identify target files, create and cryptographically verify backups, present
the exact proposed changes and rollback path, and receive explicit operator
approval.

## Agent integration contract

Labby's gateway discovery/import feature scans MCP servers already configured in
supported clients and can import them into Labby. It is not the inverse
operation of registering Labby in those clients.

The skill configures Labby into installed clients through each client's current
supported MCP interface after consulting current official documentation.
Secrets should be referenced from protected environment/credential storage
rather than embedded in tracked client configuration where supported.

The checked-in Claude plugin MCP definition is a bearer convenience because it statically emits an Authorization header. OAuth-oriented Claude clients are configured through a higher-precedence native client registration without that header, followed by Claude's supported OAuth login flow. This avoids relying on OAuth fallback behavior that Claude intentionally does not perform after an explicit Authorization header fails.

Codex App Server is treated as the Phoenix/agent-harness protocol it is. It is
not registered as an MCP upstream.

## Verification contract

A successful install requires runtime evidence:

1. installed version and service/container state;
2. `labby doctor` and readiness;
3. protected secret-file checks without printing values;
4. Code Mode state;
5. live `mcporter` transport/tool discovery;
6. one read-only Labby `help` action over MCP;
7. agent-side read-only smoke after client restart/reload;
8. relevant logs free of unresolved setup errors.

Configuration files alone are not proof of a successful deployment.

## Consequences

- Users get one memorable guided entry point while the tested Rust setup
  machinery remains the source of truth.
- Third-party UI/documentation drift can be handled by an agent that searches
  current official sources at execution time.
- The release installer stays small, deterministic, and suitable for unattended
  use.
- New setup capabilities should normally be implemented in Labby's typed
  setup/config surfaces first, then orchestrated by the skill.
- The plugin package must remain public-environment-neutral. Private hostnames,
  ports, or credentials do not belong in its defaults.
- Skill verification must be maintained alongside CLI/docs generation so
  onboarding prose cannot silently drift from executable behavior.

## References

- `plugins/labby/skills/install-labby/SKILL.md`
- `plugins/labby/README.md`
- `docs/services/SETUP.md`
- `docs/runtime/OAUTH.md`
- `docs/runtime/INCUS.md`
- `docs/PLUGINS.md`
