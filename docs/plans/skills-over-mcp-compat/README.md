# Skills over MCP compatibility implementation

Status: active implementation
Owner: Labby gateway / MCP surface
Started: 2026-08-18
Worktree: /home/jmagar/workspace/labby-skills-over-mcp
Branch: feat/skills-over-mcp

## Purpose

This folder is the living implementation package for making Labby-hosted Agent Skills usable by the widest practical set of MCP clients without forking the skill model per client.

Labby already implements the draft SEP-2640 native Skills extension: capability advertisement, skills/list, skills/get, first-party skills, local operator skills, upstream aggregation, manifest validation, digest verification, URI relabelling, route scoping, and resource reads. This project does not rebuild that foundation.

The missing layer is compatibility projection for clients that support MCP but do not natively understand SEP-2640.

## Artifacts

- SPEC.md: product and architecture specification.
- CONTRACT.md: normative compatibility and behavior contract.
- IMPLEMENTATION_PLAN.md: phased code, test, documentation, and rollout plan.
- PROGRESS.md: living status, decisions, verification log, and rebase watch. Keep this updated during every implementation session.
- ../../contracts/skills-extension.md: existing normative SEP-2640 protocol contract. Do not duplicate its wire-level requirements here.

## Core decision

One canonical skill registry, multiple projections.

1. Native SEP-2640 clients use skills/list, skills/get, and resources/read.
2. Tool-capable clients use one fixed Labby skills service with action-based list, search, get, and read operations.
3. Code Mode clients discover and invoke the same fixed skills service through the Code Mode catalog.
4. Filesystem-native clients may later receive an explicit local projection generated from the same registry.

Labby must never expose one MCP tool per skill. Skill count must not increase the MCP tool count.

## Adjacent work

The existing feature/skills-ui-config branch contains substantial work for first-class gateway Skills configuration, operator views, and loadouts. This project must not overwrite or independently reinvent those features. Integration should occur through shared types and dispatch boundaries after both branches are reviewed.

## Current upstream reference

On 2026-08-18, the modelcontextprotocol/experimental-ext-skills main branch resolves docs/sep-draft-skills-extension.md at repository snapshot f1f66fa7f8c75d6094dff1fd4a5e83f058ec8692 with file blob SHA 6b535330430f55170bab488dde661f8909fb947b. SEP-2640 remains an open Extensions Track draft in modelcontextprotocol/modelcontextprotocol PR 2640, updated 2026-08-15.

The existing Labby protocol contract intentionally pins a known revision. Any wire-level change must first update and re-run that conformance contract; compatibility projection must not silently change the native SEP surface.
