# Provider-neutral Skills core contract

Status: draft implementation contract

## Required invariants

1. Validation precedes exposure. A policy cannot make invalid content usable.
2. Exposure is enforced on list and direct get/read paths.
3. Existing absent `expose_skills` configuration means expose all; an explicit
   empty allowlist means expose none.
4. A malformed exposure policy fails closed.
5. Every validated operator entry has exactly one exposure decision.
6. `exposed` remains a derived compatibility field until all consumers migrate.
7. Hidden validated entries are not reported as ingest rejections.
8. Rejection details stay operator-only and do not leak hidden topology to a
   downstream caller.
9. Skill identity is provider-scoped; names and URI schemes are not trust or
   uniqueness signals.
10. Descriptor discovery does not fetch instruction or supporting-file bodies.
11. Remote file reads retain manifest, digest, and frontmatter verification.
12. Vendor permission metadata never grants Labby execution authority.
13. Provider calls are bounded by pages, items, bytes, deadlines, and subject
    isolation appropriate to that provider.
14. A digest match proves consistency, not authorship or trust.
15. Consuming a remote Skill does not silently create a locally owned Artifact.

## Exposure decision v1

Each validated operator entry returns:

```json
{
  "exposed": true,
  "exposure": {
    "status": "exposed",
    "reason": "matched_pattern",
    "matched_pattern": "review-*"
  }
}
```

Hidden entries use `status: "hidden"`, `reason: "not_matched"`, and no matching
pattern. An unrestricted policy uses `reason: "allow_all"` and `matched_pattern:
"*"`.

Stable reason strings are API vocabulary. Adding a reason is compatible;
renaming or changing the meaning of an existing reason requires a coordinated
consumer migration.
