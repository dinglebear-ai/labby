---
title: Skills extension contract
created: 2026-08-12
updated: 2026-08-26
---

# Skills Extension Contract (SEP-2640)

> **Status: unmerged draft.** SEP-2640 is on the MCP Extensions Track and has
> not been merged into any spec revision. Labby implements it behind the
> `skills` cargo feature so the surface can ship dark and be withdrawn if the
> draft changes incompatibly.

## Pinned revision

| What | Value |
|------|-------|
| Mirror repo | `modelcontextprotocol/experimental-ext-skills` |
| **Pinned commit** | **`9f55cd349932ba00fc18402873c9eb2d2c2e78cb`** (2026-08-04) |
| Pinned file | `docs/sep-draft-skills-extension.md` |
| Upstream provenance | PR #2640 head on branch `sep/skills-extension` |
| Upstream PR | [#2640](https://github.com/modelcontextprotocol/modelcontextprotocol/pull/2640) — open |
| Companion | `docs/threat-model.md` (T1–T9), `docs/rationale.md` |

**On the two SHAs.** Earlier planning cited `0eb05fe` as the pin. That commit
does not exist in `experimental-ext-skills` — the API returns `422 No commit
found` — because it is an *upstream* commit in the spec repo. The mirror commit
that carries that exact text is `9f55cd3`, whose message is literally
`docs: sync SEP copy with upstream commit 0eb05fe (#119)`. Pin `9f55cd3`; it is
the revision whose bytes this implementation was written against.

**Drift check as of 2026-08-18.** The mirror's current `main` snapshot is
`f1f66fa7f8c75d6094dff1fd4a5e83f058ec8692`. GitHub reports the
`docs/sep-draft-skills-extension.md` blob as
`6b535330430f55170bab488dde661f8909fb947b` at both that snapshot and the
pinned mirror commit `9f55cd349932ba00fc18402873c9eb2d2c2e78cb`. The normative SEP draft
bytes are therefore unchanged from Labby's pin. PR #2640 remains open; repeat
this comparison before changing native wire behavior.

## Resolved questions

These were open when planning started. Each is answered from the pinned text.

### 1. Is there a `list_changed` notification for skills?

**No.** SEP-2640 defines no skills-specific list-changed notification anywhere.
Hosts re-query `skills/list` to detect staleness. Per `rationale.md`, reusing
Resources means skills inherit *resource* subscriptions (the
`resourceSubscriptions` filter on `subscriptions/listen`), which is the change
mechanism available — not a skills-specific one.

**Labby:** no upstream notification registration, no downstream fan-out.
Freshness is TTL + stale-while-revalidate + gateway reload.

### 2. What are the `cacheScope` values?

**Delegated, not enumerated.** Verbatim:

> the result also carries the base protocol's list-caching attributes — `ttlMs`
> and `cacheScope`, as defined for `tools/list` and `resources/list`
> ([SEP-2549]) — with the same semantics: a freshness hint for the listing and a
> cache-scope marker, **not an integrity property**.

SEP-2549 is **merged**, and its vocabulary is exactly two values:

| Value | Meaning |
|-------|---------|
| `"public"` | No user-specific data. Any client, shared gateway, or caching proxy may store and serve it to any user. |
| `"private"` | Not meant to be shared between callers. May be reused within one authorization context; caches **must not** be shared across authorization contexts. |

Two rules that bind a paginating client: a server **must** apply the same
`cacheScope` to every page of one list, and each page carries its **own**
`ttlMs`, which servers may vary between pages.

**Labby:** `cache_scope` is typed as a free-form `Option<String>` so a future
value round-trips rather than failing to deserialize. Labby shards its cache per
OAuth subject unconditionally, regardless of the declared scope. That is
deliberately stricter than `"public"` allows: the spec names a shared gateway as
a permitted sharer and warns that a `"public"` result "may be shared between
callers even if the Result is coming from an authenticated endpoint" — sharing
we decline to do. Over-sharing is what the spec forbids; sharding more finely
than declared is always permitted and costs only upstream requests.

`cacheScope` is not an access control. Per the same document, implementations
"MUST NOT rely on `cacheScope` alone to prevent unauthorized access" — exposure
policy governs independently.

### 3. Are digest-less resources allowed?

**Yes, narrowly.** Verbatim:

> `resources` MAY be omitted only when a skill's content is generated
> dynamically, such that stable digests cannot be published. A skill without
> `resources` offers no content integrity and cannot be content-bound. **Hosts
> MAY decline to load such skills**, and server authors SHOULD expect that some
> hosts will.

**Labby declines them.** This is an explicitly spec-sanctioned host choice, not
a deviation. `SkillEntry::is_unverifiable` identifies them; ingest rejects them
as `missing_manifest` with the reason surfaced to operators.

### 4. Which JSON-RPC error codes are defined?

**Only `-32602`.** Verbatim:

> If the URI does not identify a skill the server serves, the server MUST return
> error `-32602` (Invalid params) — the same code `resources/read` uses for
> unknown resources.

Pagination-cursor errors inherit from the base protocol. Verification failure
has **no wire error code** — it is host-side logic.

### 5. What are the frontmatter fields?

Delegated wholesale to the [Agent Skills
specification](https://agentskills.io/specification).

| Field | Required | Constraint |
|-------|----------|------------|
| `name` | yes | ≤64 chars; lowercase letters, digits, hyphens; no leading/trailing hyphen; **no consecutive hyphens**; equals the parent directory name |
| `description` | yes | ≤1024 chars, non-empty |
| `license` | no | string |
| `compatibility` | no | ≤500 chars |
| `metadata` | no | object, string values only |
| `allowed-tools` | no | space-separated **string** (not a list); experimental |

Unknown keys pass through unchanged — the listing carries frontmatter "verbatim
as a JSON object — every field the author wrote, not a curated subset". Keys
under the reserved `io.modelcontextprotocol/` prefix inside `metadata` are
ignored when unrecognized.

### 6. Are there cross-skill references?

**No dependency field exists.** T9 nesting is *file-based*: a skill directory may
contain descendant skills, and from the enclosing skill's perspective those are
ordinary supporting files. Verbatim:

> A nested `SKILL.md` read this way is ordinary markdown: hosts MUST NOT act on
> its frontmatter.

**Labby:** nested `SKILL.md` files are inert bytes. Labby never auto-fetches a
referenced skill and never gives effect to a nested skill's frontmatter.

## Requirements Labby implements

### Manifest completeness and binding

When present, `resources` MUST be complete — every file, **each exactly once**,
**including an entry matching the skill's own `uri`** carrying `SKILL.md`'s
digest. Each URI MUST be the skill's `SKILL.md` or a file within the skill's
directory.

Within one entry a repeated URI is invalid regardless of digest agreement.
*Across* entries duplicates are legal and expected: "the same file may appear in
both the enclosing and the nested skill's entries."

### Reads are manifest-bound

> while acting on a skill for which the host holds an entry, a host MUST resolve
> reads of the skill's files only to URIs listed in that entry's `resources`,
> and MUST treat a read of an unlisted file within the skill as **a verification
> failure equivalent to a digest mismatch**.

The two failures are spec-equivalent, and both recover the same way: refresh via
`skills/get` (or `skills/list`) and proceed from the current `resources` set,
"which, being different, revokes any content-bound approval."

### Frontmatter cross-verification

> After fetching a `SKILL.md` for which the host holds an entry … hosts MUST
> parse its YAML frontmatter and compare it field-by-field against the entry's
> `frontmatter`. Any discrepancy MUST be treated as a verification failure
> equivalent to a digest mismatch, and the skill MUST NOT be loaded.

`compare_frontmatter` implements this as a total, exact comparison — a key on one
side and not the other is a discrepancy, which is what makes it catch a
`SKILL.md` that quietly grants itself `allowed-tools` the approved entry never
carried.

### Unenumerated skills are first-class

> A server is not required to make its skills enumerable. A skill's URI is
> directly readable via `resources/read` whether or not it appears in any
> listing, and **hosts MUST support loading a skill given only its URI**.

> Hosts MUST NOT treat an empty or partial listing as proof that a server has no
> skills.

Absence from a cached listing therefore is **not** an error. Labby resolves a
cache miss by calling `skills/get` for that URI; only `-32602` means
not-a-skill.

### Per-origin namespacing (T8)

> When skills from different origins collide on `name`, hosts MUST resolve the
> name within a per-origin namespace, identifying servers by a **host-assigned
> label**; an MCP-served skill MUST NOT silently shadow, or be silently
> substituted for, a same-named skill from any other origin.

Labby's origin-label prefix is this mechanism. Names are labels, not
identifiers — two skills in one server may share a name (`acme/billing/refunds`
and `acme/support/refunds`), so deduplication keys on full URI, never on name.

### Identity does not come from the scheme

> A host MUST NOT conclude that a resource is a skill merely because its URI
> carries a particular scheme.

Identity comes from a `skills/list` entry or a `skills/get` confirmation.

## What digest verification does not prove

> Digests are unsigned and supplied by the same server that supplies the
> content. A match proves the two are consistent, not that either is
> trustworthy. **Any intermediary on the path, such as a gateway, can rewrite
> both the listing and the content together. Hosts MUST NOT treat a digest match
> as a security boundary.**

Labby is exactly that intermediary. Verification catches inconsistency,
corruption, and staleness. Describe it as a consistency check — never as tamper
detection, and never as a security boundary.

### T3 residual: `allowed-tools` through a gateway

A skill's `allowed-tools` names tools in *its origin's* namespace. Downstream of
Labby the catalog is aggregated, so the names may resolve against a different
server's tools or against Labby's own privileged tools. Labby emits a structured
origin label on every aggregated entry so a client can scope the field to its
origin.

Each aggregated entry carries an `ai.dinglebear.labby/skillOrigin` block in
`_meta` naming the origin, how its tools are reachable (`direct` or
`code_mode_only`), and — when direct — the downstream tool names it accounts
for. Under Code Mode those names do not exist, so the list is omitted rather
than emitted empty.

This rides in `_meta`, never in `frontmatter`, because the SEP requires
frontmatter to be verbatim and requires hosts to refuse a skill whose
frontmatter disagrees with its `SKILL.md`.

## URI grammar

```
skill://<origin-label>/<skill-path>/<file-path>
```

The SEP's own form is `skill://<skill-path>/<file-path>`, where `<skill-path>`
is **one or more** segments whose **final** segment is the skill's `name`, and
any preceding segments are a server-chosen organizational prefix. The SEP is
explicit that the first segment, though it occupies the RFC 3986 authority
component, **"carries no special semantics under this convention."**

Read that carefully before touching this code. Treating the first segment as a
routing authority — rather than as part of `<skill-path>` — is the exact
divergence the SEP's Motivation cites as the reason it exists: implementations
"invented their own `skill://` URI structure, with diverging semantics for
authority, path, and sub-resource addressing." Labby made that mistake, and its
cost was concrete: `skill://git-workflow/SKILL.md` — the SEP's own first
example — was rejected at ingest, so every one-segment skill from a conforming
upstream was silently dropped.

Labby therefore **prepends** its host-assigned label as an additional prefix
segment rather than claiming the authority slot:

| Upstream serves | Labby publishes | Skill name |
|---|---|---|
| `skill://git-workflow/SKILL.md` | `skill://gh/skill/git-workflow/SKILL.md` | `git-workflow` |
| `skill://acme/billing/refunds/SKILL.md` | `skill://gh/skill/acme/billing/refunds/SKILL.md` | `refunds` |

Prepending preserves the name-is-the-final-segment invariant at any depth and is
**lossless**: stripping the label recovers the upstream's URI exactly, which is
what lets a proxied read be routed back. Replacing the first segment was lossy —
it discarded `acme` above — and is what `SkillUri::with_origin` used to do.

Relabelling at all is required, not cosmetic: a skill is identified by its
`uri`, so two upstreams serving `skill://git-workflow/SKILL.md` passed through
unchanged would publish one identifier twice, and neither Labby nor the
downstream host could route or disambiguate. The SEP separately requires hosts
to resolve names in a per-origin namespace under a host-assigned label and
forbids one origin's skill shadowing another's.

Provenance rides in `_meta` under Labby's own reverse-domain prefix, which the
SEP explicitly sanctions: "Intermediaries MAY attach provenance or verification
annotations via `_meta` under their own reverse-domain prefix — not the
`io.modelcontextprotocol.skills/` prefix reserved for this extension."

`labby` is reserved for first-party skills. Under prepending this is
structurally guaranteed rather than merely validated: an upstream's segments can
never occupy the first position of a URI Labby publishes.

**No scheme is privileged.** An upstream MAY serve skills under a scheme
native to its own domain — the SEP's example is
`github://owner/repo/skills/refunds/SKILL.md` — and "the structural constraints
above ... apply regardless of scheme." Labby parses any RFC 3986 scheme and
applies the same structure; requiring `skill://` silently excluded every skill
from a conforming upstream that used its own.

Two consequences follow, both handled by refusing rather than guessing:

- A manifest may not mix schemes. Every file of a skill lives in that skill's
  directory, so one scheme per skill; a cross-scheme entry excludes the skill.
- Minting preserves the upstream's scheme as the segment immediately after the
  gateway label while publishing in Labby's `skill://` namespace. This keeps
  native-scheme URIs distinct and makes the mapping exactly reversible.

The native URI is reconstructed by removing Labby's label and decoding the
preserved scheme segment. Cached manifests remain authoritative for digest and
ownership checks, but an uncached `skills/get` can route without guessing. Note
also that the scheme confers no identity: the SEP says a host
"MUST NOT conclude that a resource is a skill merely because its URI carries a
particular scheme" — identity comes from a `skills/list` entry or `skills/get`.

**Inbound URIs are not held to Labby's minting grammar.** Labby mints labels as
lowercase-alphanumeric-with-hyphens, but the SEP only says the first segment
SHOULD be a valid RFC 3986 `reg-name`. Applying the stricter minting rule to
upstream URIs rejected conforming servers; the grammar is enforced in
`with_origin`, not in `parse_skill_uri`.

**The skill/file split is a manifest lookup, never positional.** Given
`skill://labby/pdf-processing/references/FORMS.md` in isolation, the skill could
be `pdf-processing` or `pdf-processing/references`; both are well-formed. The
one exception is the canonical `.../SKILL.md` form, which the SEP makes explicit
so "the skill name is always recoverable from the URI alone, without reading
frontmatter" — exposed as `SkillUri::skill_md_parts`.

## Wire shapes

Capability declaration (empty object = supported, no optional features):

```json
{ "capabilities": { "extensions": { "io.modelcontextprotocol/skills": {} } } }
```

Clients MUST NOT call `resources/directory/read` against a server that has not
declared `directoryRead: true`. Labby does not declare it.

`skills/get` returns its entry **nested under a `skill` key** — not flat:

```json
{
  "result": {
    "skill": {
      "uri": "skill://pdf-processing/SKILL.md",
      "frontmatter": { "name": "pdf-processing", "description": "…" },
      "resources": [
        { "uri": "skill://pdf-processing/SKILL.md", "digest": "sha256:d5e6f7a8…" }
      ]
    }
  }
}
```

`skills/list` returns `skills[]` plus `nextCursor`, `ttlMs`, and `cacheScope`.
Digests are `sha256:{hex}` with exactly 64 lowercase hex characters.

## Labby-owned Skill Library extension

SEP-2640 defines discovery and reading; it does not define authoring,
revisioning, activation, visibility, or import. Labby keeps those product
operations outside the SEP namespace as actions on its existing `skills` tool:

- read metadata: `skill_library.list`, `.get`, `.history`, `.read`;
- author without implicit activation: `.validate`, `.create`, `.save`;
- publish exact revisions: `.activate`, `.deactivate`, `.rollback`, `.refresh`;
- acquire without implicit activation: `.import`;
- retain revisions while retiring catalog visibility: `.archive`.

These actions are a Labby extension and must not be presented as SEP-2640
methods. They share the same immutable published generation as native
`skills/list`, `skills/get`, and `resources/read`, and as the compatibility
`skills.list`, `skills.get`, and `skills.read` actions.

The management surface is versioned and optimistic: mutations require an
expected library version and idempotency key, and revision-sensitive mutations
require an exact expected revision. A successful receipt distinguishes durable
commit from process publication and tells clients to re-list. There is no
Skills-specific list-changed notification.

Authorization is caller-dependent. Personal entries are owner/admin-only;
shared entries are company-readable only when active; owner and current admins
may mutate. This is an access-control rule, not a `cacheScope` interpretation.
The canonical lifecycle, storage, import, and MCP App behavior is documented in
[Skills And Skill Library](../services/SKILLS.md).

## Error kinds

| Kind | Origin | Recovery | Same args | Side effects |
|------|--------|----------|-----------|--------------|
| `skill_digest_mismatch` | `validation` | `rediscover` | `never` | `none_expected` |
| `skill_manifest_stale` | `validation` | `rediscover` | `never` | `none_expected` |

`rediscover` rather than `do_not_retry` because the SEP's own prescribed recovery
is to refresh the entry and proceed from the current `resources` set, and it
names benign staleness — the skill was updated after the listing was fetched — as
a normal cause alongside corruption. `never` still forbids replaying the
identical read: an unchanged retry cannot succeed until the entry is refreshed.
The generic `rediscover` guidance is overridden, since it points at actions,
tools, prompts, and resources — none of which is the method a caller needs.

## Budgets

Host-chosen, not spec-mandated; SEP-2640 defines no limits and its threat model
(T6) puts the responsibility on the host. Defined in
`crates/labby-runtime/src/skills/limits.rs`.

| Budget | Value |
|--------|-------|
| Skills per upstream | 256 |
| Resources per skill | 512 |
| Total raw bytes per skill | 16 MiB |
| Raw bytes per resource | 16 MiB |
| `skills/list` pages | 16 |
| `skills/list` wall clock | 10s |
| Frontmatter bytes | 16 KiB |
| URI segment / whole URI | 128 / 1024 chars |

Count caps MUST be applied **incrementally per page**. rmcp's own
`list_all_resources()` accumulates every page with no cap, so copying that shape
would let one upstream stream unbounded pages inside the wall-clock budget
before any limit engaged.

## Drift protection

`crates/labby-runtime/tests/skills_contract_conformance.rs` binds this document
to the code: the pinned SHA, the error-kind table, and the budget table are all
asserted against their Rust definitions, so editing one without the other fails
CI.

A scheduled drift watcher (an extension of `scripts/ci/mcp_upstream_drift.py`)
compares the pinned SHA against the mirror's HEAD and opens an issue on
divergence. **That issue must be triaged before any release touching skills
aggregation** — a warn-only job nobody reads defeats the point of pinning.
