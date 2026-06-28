# MCP Registry Lab Metadata

This document defines Lab-owned registry metadata layered on top of the mirrored MCP Registry `/v0.1` surface.

Lab does not mutate upstream registry fields. All Lab-owned metadata lives under:

```json
"_meta": {
  "dev.labby/registry": { ... }
}
```

## Ownership

- upstream metadata remains the source of truth for official registry lifecycle fields
- Lab metadata is stored locally in a separate table
- the two are merged at read time

## Contract

The Lab metadata contract is intentionally strict for first-class fields.

Supported top-level sections:

- `curation`
- `trust`
- `quality`
- `security`
- `ux`
- `audit`
- `extra`

`audit` is Lab-managed and cannot be supplied by callers.
If callers need non-core metadata, it must go under `extra`.

## Field Shape

```json
{
  "curation": {
    "featured": true,
    "hidden": false,
    "tags": ["recommended", "stable"],
    "notes": "Works well in small homelab setups."
  },
  "trust": {
    "reviewed": true,
    "reviewed_at": "2026-04-23T15:00:00Z",
    "source_verified": true,
    "maintainer_known": false
  },
  "quality": {
    "install_tested": true,
    "last_install_tested_at": "2026-04-23T15:00:00Z",
    "transport_score": "good"
  },
  "security": {
    "ssrf_reviewed": true,
    "permissions_reviewed": true,
    "secrets_reviewed": true
  },
  "ux": {
    "works_in_lab": true,
    "recommended_for_homelab": true,
    "setup_difficulty": "easy"
  },
  "extra": {
    "review_source": "manual"
  }
}
```

## Validation Rules

- payload must be a JSON object
- unknown top-level fields are rejected unless placed under `extra`
- `audit` is rejected on write
- `trust.reviewed_at` must be RFC3339 when present
- `quality.last_install_tested_at` must be RFC3339 when present
- `quality.transport_score` must be one of:
  - `good`
  - `mixed`
  - `poor`
- `ux.setup_difficulty` must be one of:
  - `easy`
  - `medium`
  - `hard`
- `curation.tags` must not contain empty values
- tags are trimmed and deduped on write
- blank optional strings are normalized away on write

## Audit Fields

Lab injects audit metadata on read:

```json
"audit": {
  "updated_at": "2026-04-23T15:00:00Z",
  "updated_by": "gateway-admin"
}
```

Current writer labels:

- `lab-cli`
- `gateway-admin`
- `unknown`

Future surfaces should set a stable actor label when writing metadata.

## Server-Side Filters

The mirrored `/v0.1/servers` surface supports server-side Lab metadata filters:

- `featured`
- `reviewed`
- `recommended`
- `hidden`
- `tag`

These filter only on Lab-owned metadata and do not affect upstream official fields.

## CLI Surface

Registry metadata is operated through the unified Marketplace CLI surface:

```bash
labby marketplace mcp.meta.get --params '{"name":"io.github.user/server","version":"latest"}'
labby marketplace mcp.meta.set --params '{"name":"io.github.user/server","metadata":{"curation":{"featured":true},"trust":{"reviewed":true}}}'
labby marketplace mcp.meta.delete --params '{"name":"io.github.user/server","version":"latest"}'
```

For advanced cases, include the full metadata object in `--params`.

## UI Surface

The gateway-admin registry UI:

- shows Lab metadata badges and filters
- uses server-side registry filters
- provides a structured editor for first-class fields
- keeps CodeMirror available under advanced JSON

## Storage Rules

- local Lab metadata is stored separately from upstream server blobs
- upstream sync must remain idempotent
- metadata writes must not rewrite or corrupt upstream `_meta`
- read paths merge official upstream metadata with Lab metadata at response time
