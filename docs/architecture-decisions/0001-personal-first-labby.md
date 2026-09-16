---
title: "ADR-0001: Personal-first Labby with optional Team distribution"
created: "2026-09-15"
updated: "2026-09-15"
status: "accepted"
---

# ADR-0001: Personal-first Labby with optional Team distribution

## Context

Labby supports both personal deployments and shared team-hosted infrastructure. These two shapes solve different problems and must not be collapsed into a central-server requirement.

The primary product goal for an individual user is a personal Labby that they control and can connect to their own local or remote tool runtimes, including Claude Code, and expose to their own ChatGPT account through OAuth. This personal runtime remains useful even when a user cannot or does not want to route work through a team-hosted Labby.

A team may additionally operate a hosted Team Labby and Team Depot. That shared deployment exists primarily to publish, govern, and distribute team-owned artifacts, server definitions, loadouts, skills, prompts, resources, and approved MCP integrations. Team members can discover and pull those assets into their personal Labby.

## Decision

1. **Personal Labby is the primary runtime.** A user's personal Labby owns that user's runtime configuration, OAuth setup, personal credentials, Claude Code MCP connections, ChatGPT connection, personal upstreams, local state, and user-selected artifacts.
2. **Personal OAuth is first-class.** The supported personal setup path includes Google OAuth on the personal Labby so the user can connect that Labby to their ChatGPT account.
3. **Claude Code attaches to personal Labby.** Local or remote Claude Code MCP is configured as an upstream of the user's personal Labby unless the user explicitly chooses another topology.
4. **Team Labby is optional shared infrastructure.** A Team Labby is not required for personal Labby operation, authentication, Claude Code access, ChatGPT access, or personal project work.
5. **Team Labby is a distribution and governance source.** Its primary role is to house and publish team-approved artifacts and server definitions that personal Labby instances can acquire, sync, fork, pin, or enable under the user's own runtime and policy.
6. **Team Depot is the default team artifact source when a user enrolls in an organization profile.** Enrollment may seed Team Depot and other team-approved MCP/server definitions into personal Labby, but it must not transfer runtime ownership away from the personal instance.
7. **Organization bootstrap is additive.** Joining a team imports organization defaults into personal Labby. It does not turn the personal installation into a thin client of Team Labby.
8. **No hidden central dependency.** Product features must not silently require a reachable Team Labby when their documented scope is personal operation. If a feature truly requires team authority, that requirement must be explicit in its contract and UI.

## Consequences

- The default setup documentation and WebUI onboarding prioritize a complete personal Labby deployment first.
- Google OAuth, public HTTPS, Claude Code MCP, ChatGPT connectivity, doctor/readiness, recovery, and support diagnostics must all work without Team Labby.
- Team enrollment can be a later optional step and should be resumable or skippable.
- Team-provided server definitions and artifacts are copied or referenced through explicit provenance-aware acquisition rather than implicitly executed from the team control plane.
- Personal configuration remains inspectable and reversible even after organization bootstrap.
- Team administrators can publish defaults, invitations, and signed bootstrap profiles without gaining automatic access to a member's local credentials or personal Labby state.

## Guardrails

A proposed setup or architecture change violates this ADR if it does any of the following without an explicit new decision:

- makes Team Labby mandatory for personal OAuth, Claude Code, ChatGPT, or normal personal runtime operation;
- stores a user's personal provider secrets only on Team Labby;
- requires personal MCP calls to transit Team Labby merely because the user is enrolled in a team;
- prevents a user from disabling or removing a team-provided server or artifact from their personal Labby;
- makes organization enrollment destructive to pre-existing personal configuration.

## Related contracts

- [Architecture](../ARCH.md)
- [Setup](../services/SETUP.md)
- [Access Control, Workspaces, and Artifact Distribution](../access-control/README.md)
- [Artifacts and Agent Skills](../services/SKILLS.md)
- [Depot control-plane contract](../contracts/depot-control-plane.md)
