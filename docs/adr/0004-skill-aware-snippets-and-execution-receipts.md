---
title: "ADR 0004: Resolve Snippet Skills through Host Policy and Retain Execution Receipts"
created: "2026-09-20"
updated: "2026-09-20"
---

# ADR 0004: Resolve Snippet Skills through Host Policy and Retain Execution Receipts

Date: 2026-09-20

Status: Proposed

## Context

Labby can search and read mounted skills, discover external `skills.sh`
candidates, execute snippets, journal explicitly selected Code Mode steps, and
promote ephemeral snippet source through an authorized native action. These
facilities do not yet form one reproducible workflow.

External search results cannot be safely previewed without ingestion. Snippets
do not declare a first-class skill resolution policy. QuickJS receives complete
tool results but is intentionally memory-bounded and short-lived, while the step
journal retains only values explicitly checkpointed within tighter limits.
Promotion therefore lacks a durable record of the skills and upstream results
that produced a successful dynamic execution.

Loadouts already define reusable capability projections. Adding instruction
content directly to Loadouts would mix authority selection with specialist
composition and risk creating a second authorization model.

## Decision

Saved snippets declare skill resolution policy and produce durable execution
receipts owned by the host. Skill content informs execution but never grants or
widens authority.

### Resolve skill content without changing trust

Labby provides a read-only external-skill preview operation keyed by the
immutable candidate identity returned by discovery. It returns bounded
`SKILL.md` content, provenance, source, and digest without installing, ingesting,
or executing the candidate or its bundled resources. External content is
explicitly marked untrusted or candidate and remains subject to caller and route
policy, byte limits, and time limits.

Code Mode exposes an ergonomic skill-content helper while preserving the
distinction between a lightweight skill manifest and the skill body.

### Make snippet dependencies dynamic or pinned

A snippet may define dynamic skill search, ranking, source, trust, and candidate
limits, or pin exact skill identities and digests. A successful dynamic run
records the resolved identities, origins, trust classifications, and content
digests. An authorized promotion may freeze that receipt into pinned
dependencies.

Effective tool authority is always the intersection of the caller, route,
Loadout or equivalent capability projection, and snippet policy. Skill text,
frontmatter, requested tools, and external popularity cannot enlarge that
intersection.

### Retain execution evidence outside QuickJS

For saved snippet executions, the host records a durable execution receipt that
can reference:

- execution identity and the exact snippet revision or digest;
- resolved skills with provenance, trust, and digest;
- tool-call identity and a hash of the parameters;
- full-result handles with content type, size, and hash;
- timestamps, duration, status, errors, step-journal rows, final output, and
  exported artifacts.

Full upstream responses use the retained-result store and opaque handles defined
by issue #274. Labby does not create a parallel blob store for snippets.
Receipts and results are caller, team, and route scoped; have bounded retention
and quotas; redact searchable metadata; protect sensitive raw values; and return
structured expired or evicted errors.

QuickJS remains the active execution cache. Small values can materialize in the
sandbox. Large values spill to host-owned handles before realistic workloads
approach the sandbox heap limit.

### Query retained results through bounded host operations

Code Mode can inspect statistics and perform bounded structural selection, text
search, slicing, and execution-wide search over retained handles without
rehydrating the complete value. Exact and text search are sufficient for the
initial contract; vector search is not required.

The final model-visible result still follows the reduce-before-return contract
from issue #217. Durable retention supports replay, audit, reflection, and
promotion; it does not justify returning unbounded data to the model.

### Compose specialists without merging instructions and authority

An ephemeral specialist consists of a Loadout or equivalent authority
projection plus resolved skills, prompt and resource bindings, and task input.
If reusable instruction composition needs a durable name, it uses a thin
execution-profile layer that references a Loadout. Loadouts remain capability
projections.

Promotion stays host or caller mediated through the authorized promotion
surface. Sandbox code cannot promote itself or convert a successful execution
into new authority.

## Consequences

- Dynamic workflows can be audited and promoted reproducibly after the QuickJS
  instance exits.
- Retention adds quota, encryption or protection, eviction, and lifecycle work
  to the existing result-handle subsystem.
- Skill discovery and content reading become easier without treating discovery
  as installation or trust.
- Large results can remain useful to Code Mode without consuming the entire
  sandbox heap or final response budget.
- Specialist composition reuses the existing capability projection instead of
  creating a competing permissions system.

## Alternatives considered

### Store all upstream responses in the step journal

Rejected because the journal is an explicit bounded checkpoint facility and has
different size and retention semantics from complete execution evidence.

### Keep complete results in QuickJS until the run ends

Rejected because it makes useful workload size depend on a small execution heap
and loses the evidence at process exit.

### Create a snippet-specific result database

Rejected because issue #274 already establishes retained-result handles and a
second store would duplicate ownership, quotas, and expiration behavior.

### Let a skill request additional tools

Rejected because untrusted instructions cannot be an authority source.

### Put skills and prompts directly into Loadouts

Rejected as the default model because Loadouts already represent capability
authority. A referencing execution profile keeps those roles explicit.

## Authority and implementation status

This ADR records the proposed architecture from [GitHub issue
#708](https://github.com/dinglebear-ai/labby/issues/708). It does not claim that
external previews, retained execution receipts, result search, specialist
profiles, or pinned promotion are implemented.

## References

- [GitHub issue #708](https://github.com/dinglebear-ai/labby/issues/708)
- [GitHub issue #274](https://github.com/dinglebear-ai/labby/issues/274)
- [GitHub issue #217](https://github.com/dinglebear-ai/labby/issues/217)
- `crates/labby-codemode/src/`
- `crates/labby-gateway/src/codemode_journal/`
- `docs/guides/SKILLS_AND_LOADOUTS.md`
- `docs/services/SNIPPETS.md`
