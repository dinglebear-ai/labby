# Compatibility contract: Skills projection

Status: normative for this implementation
Created: 2026-08-18
Last updated: 2026-08-18

This contract governs Labby's compatibility projections for Agent Skills. The native SEP-2640 wire contract remains docs/contracts/skills-extension.md. Where the two overlap, the native SEP contract wins for native extension requests.

## 1. Invariants

### C-1 One canonical registry

All projections resolve against the same canonical skill identities and verified content. A compatibility adapter may change shape, never meaning.

### C-2 One fixed compatibility tool

Labby exposes at most one compatibility MCP tool/service named skills. Skill count must never create additional tools.

### C-3 Native and compatibility identity parity

For any visible skill URI U:

- native skills/get(U) and compatibility skills.get with URI U resolve the same SkillEntry
- compatibility skills.read for file F and native resources/read(F) return the same verified text bytes for the same visible file URI F

### C-4 Progressive disclosure

list and search return metadata only. They must not inline SKILL.md or support-file bodies.

### C-5 Route parity

A caller may discover/get/read only origins allowed by that caller's route scope. An excluded origin is reported as absent rather than revealing hidden topology.

### C-6 Auth parity

Compatibility read operations require the same effective read authorization as the native Skills extension and skill resource reads.

### C-7 Verification parity

Compatibility reads of proxied content use the existing manifest-bound, digest-verified upstream read path. No second fetch path may bypass verification.

### C-8 Honest partial results

Aggregate list/search results preserve incomplete-state annotations when upstream failures, rejected entries, or budget truncation prevent a complete view.

### C-9 Stable tool cardinality

A client with 1, 1,000, or 100,000 visible skills sees the same compatibility action catalog.

## 2. Shared action catalog

The service name is skills. Every action also supports the standard Labby help and schema built-ins through shared dispatch.

### 2.1 skills.list

Purpose: enumerate compact visible skill entries.

Parameters:

- origin: optional string; restrict to one visible origin label
- limit: optional integer; default 100, minimum 1, maximum 500
- cursor: reserved for future pagination; P0 may reject non-null values until a stable shared cursor exists

Result object:

- skills: ordered array of SkillSummary
- incomplete: optional object copied or derived from canonical aggregate state
- total_returned: integer
- truncated: boolean

SkillSummary fields:

- uri: published skill URI
- name: Agent Skills name from frontmatter
- description: description from frontmatter
- origin: Labby-visible origin label
- frontmatter: complete frontmatter object as published by the canonical entry
- resources: optional compact resource manifest; each element has uri and digest
- provenance: optional Labby metadata already safe for the caller

Ordering is deterministic: origin, name, URI.

### 2.2 skills.search

Purpose: find likely skills without loading their bodies.

Parameters:

- query: required non-empty string
- origin: optional string
- limit: optional integer; default 20, minimum 1, maximum 100

P0 scoring, highest first:

1. exact case-insensitive name match
2. case-insensitive name prefix match
3. case-insensitive name substring or token match
4. description substring or token match
5. string-valued metadata match

Tie order: origin, name, URI.

Result object:

- query: normalized query text
- matches: array of SkillSearchHit
- incomplete: optional object
- total_returned: integer

SkillSearchHit contains score, match_fields, and skill.

Search must not fetch SKILL.md bodies solely to improve ranking.

### 2.3 skills.get

Purpose: resolve one visible skill entry.

Parameters: uri, required published URI.

Result object: skill, a SkillSummary using the canonical current entry.

An unknown URI returns a structured not-found/discovery error. A route-hidden URI behaves as unknown.

### 2.4 skills.read

Purpose: load one file belonging to a visible skill.

Parameters: uri, required published file URI.

P0 result object:

- uri: published file URI
- skill_uri: published SKILL.md URI that binds this file
- origin: visible origin label
- mime_type: optional string
- text: verified text content
- digest: digest from the current manifest

P0 is text-only because the existing canonical verified upstream read path currently returns text resource contents. A non-text skill resource must fail with a structured unsupported-content error rather than being silently corrupted or decoded under an undocumented shape. Binary support is a later contract revision.

A read must fail when the URI is not owned by a visible skill, the file is not listed in the current manifest, the fetched digest disagrees with the manifest, the current entry no longer binds the file, or the upstream returns an unsupported content shape.

Digest or manifest mismatch uses Labby's existing skill verification recovery semantics: rediscover and retry against the refreshed entry.

## 3. Native extension projection

Native io.modelcontextprotocol/skills requests continue to use their existing methods and wire DTOs. The compatibility service must not alter extension advertisement, native response shapes, native JSON-RPC error codes, skill URI grammar, or digest/frontmatter verification rules.

Shared implementation may be extracted underneath these handlers, but behavior must remain semantically compatible unless the pinned SEP contract is deliberately updated.

## 4. Code Mode projection

The skills service is part of the internal service registry when the skills feature is enabled. If direct tools are hidden by Code Mode, callers discover, describe, and call the same skills service through Code Mode. No separate Code Mode skill index may exist.

## 5. CLI and API projection

CLI and API are thin adapters over the same shared dispatch action catalog.

Planned CLI shape:

- labby skills list
- labby skills search QUERY
- labby skills get URI
- labby skills read URI

The API uses the repository's established per-service route convention and calls shared dispatch only. Human CLI formatting may differ; JSON semantics may not.

## 6. Filesystem projection contract: deferred

A future filesystem projector must be explicit opt-in, keep the canonical registry as source of truth, verify every projected file before staging, write to a staging directory then atomically activate, preserve a provenance manifest, avoid overwriting unmanaged local skills without explicit policy, support dry-run, never project route-hidden skills, and never treat a digest as a trust signature.

Client-specific path discovery belongs in adapters, not the core registry.

## 7. Error contract

Compatibility actions return the existing versioned agent error envelope.

Required behavior:

- missing or invalid param: revise_and_retry, no side effects
- unknown skill or file: rediscover, no side effects
- skill_digest_mismatch: rediscover
- skill_manifest_stale: rediscover
- upstream unavailable: retry_later when the operation requires that upstream
- unsupported content: inspect_and_escalate or revise_and_retry depending on whether another URI can satisfy the request

List/search should prefer partial success plus incomplete metadata over failing the entire request because one upstream is down.

## 8. Limits

P0 compatibility limits:

- list maximum 500 entries per call
- search maximum 100 matches per call
- one file per read call
- existing upstream Skills traversal and fetch budgets remain authoritative
- no implicit batch resource reads

If a response would exceed normal MCP or Code Mode envelope budgets, callers narrow list/search or read files individually.

## 9. Observability

Every action emits one completion event at the shared dispatch boundary with surface, service=skills, action, elapsed_ms, outcome or error kind, redacted caller subject where applicable, visible origin where applicable, and result count for list/search.

Never log skill file bodies, tokens, authorization headers, or unredacted subject identifiers.

## 10. Compatibility definition

Labby may describe a client as native, tool-compatible, code-mode-compatible, resource-compatible, or filesystem-compatible. Only native means native Skills-over-MCP support. The other labels must not be presented as protocol-native support.
