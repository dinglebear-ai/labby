# Specification: provider-neutral Skills core

Status: active
Created: 2026-08-22

## Problem

The current upstream Skills path validates, caches, filters, and serves draft
SEP-2640 entries safely. Its operator model still collapses a valid skill's
availability to `exposed: bool`, and its policy reuses the tool-name allowlist
type. Neither representation can explain why a skill is unavailable or model
providers, provenance, compatibility, requirements, and loadout selection.

The observed hidden population is primarily validated content that did not
match `expose_skills`; it is not an ingest-rejection population. Relaxing
validation or globally exposing every skill would solve the wrong problem.

## Goals

1. Define one canonical, provider-neutral Skill descriptor and identity.
2. Preserve progressive disclosure: descriptor first, instructions on
   activation, supporting files only when used.
3. Make availability and exposure decisions explicit and operator-readable.
4. Keep activation separate from execution authorization.
5. Preserve existing `expose_skills = ["name", "prefix-*"]` behavior.
6. Treat SEP-2640, local packages, bundled skills, and future registries as
   providers/adapters rather than competing Skill models.
7. Allow remotely consumed skills to remain remote; taking local ownership is
   a separate Artifact operation.

## Non-goals

- Eagerly importing or downloading every discovered package.
- Treating a digest as proof that an author is trustworthy.
- Granting tools, shell, network, filesystem, or secrets from vendor metadata.
- Replacing the existing SEP-2640 wire contract or compatibility projection.
- Exposing every validated skill by default.

## Model

### Identity

A Skill identity is scoped by its provider and source identity. For an MCP
provider this includes the host-assigned upstream identity and source URI. A
name is a display/search field, never a globally unique identifier.

### Descriptor

The compact descriptor contains identity, name, description, provider,
provenance, revision/integrity metadata when available, compatibility summary,
requirements summary, and availability. It does not contain `SKILL.md` or file
bodies.

### Provider

A provider performs bounded descriptor discovery and lazy package/file reads.
Providers normalize source-specific data into the canonical descriptor and
preserve uninterpreted vendor metadata. The SEP-2640 adapter retains the
current pagination, cache-scope, subject isolation, manifest, digest, and
frontmatter verification behavior.

### Availability and exposure

Availability describes whether Labby can safely offer a Skill. Initial states:

- `validated_exposed`
- `validated_hidden`
- `integrity_rejected`
- `transport_unsupported`
- `dependency_unavailable`
- `policy_blocked`
- `truncated`

An exposure decision records whether the Skill is visible, the stable reason,
and the matching rule when one exists. The initial implementation distinguishes
`allow_all`, `matched_pattern`, and `not_matched`; later policy layers may add
loadout, project, group, and explicit deny decisions.

### Permission boundary

Discovery, exposure, or activation never authorizes execution. Source metadata
such as Claude's `allowed-tools` is preserved as a compatibility requirement or
hint. Labby's ordinary authorization and destructive-action rules remain the
only authority for tool calls and side effects.

### Artifact boundary

A remote Skill may be consumed lazily from its provider. It becomes a locally
owned Artifact only for explicit operations such as save, fork, customize,
share, or pin revision.

## First milestone

Replace the operator-only boolean with a structured exposure decision, project
that decision through `gateway.skills.list`, retain the legacy `exposed` field,
and show an exact reason/rule for every validated skill. This unlocks safe
operator management of currently hidden skills without changing what callers
can access.
