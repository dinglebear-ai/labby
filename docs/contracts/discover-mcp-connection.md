# Discover MCP connection metadata

A catalog may attach `mcpConnection` to an artifact list or detail response:

```json
{
  "mcpConnection": {
    "schemaVersion": "labby.mcp-connection/v1",
    "revisionId": "the-current-revision-id",
    "transport": "http",
    "authentication": "none",
    "url": "https://example.org/mcp"
  }
}
```

The revision must equal `currentRevisionId` (or `currentRevision.id`). This version accepts only HTTPS endpoints without embedded credentials, query strings, fragments, whitespace, control characters, or backslashes. Authentication is `none` or `bearer`. Endpoint text is limited to 2,048 bytes; revision IDs are limited to 512 bytes. Unknown fields, transports, and authentication modes are unsupported. Invalid metadata is omitted by the gateway projection while the catalog entry remains readable.

This is connection input, not execution authority or a publisher trust statement. The user reviews the exact endpoint and explicitly approves connecting and exposing tools. Bearer credentials are collected by a protected input and sent through the existing gateway credential writer. The existing gateway runtime owns authorization, network policy, persistence, and connection probing. Catalog metadata never supplies a shell command or a tool call.

Activation creates an HTTP upstream with resources, prompts, Skills, and MCP UI proxying disabled. A saved upstream survives failed probing and can be retried through its retained identity. A successful connection probe verifies connectivity and reports tool discovery; it does not establish successful tool invocation or route exposure. The next screen lists actual exposed runtime tools that explicitly declare read-only behavior with supported scalar inputs. The form populates enum choices and validates required string, boolean, integer and number fields; unsupported nested or composition schemas remain outside this fast path. The user reviews the named tool, inputs and destination and explicitly approves one call. Server annotations are hints, not trust decisions. The tool list returns a review fingerprint covering the schema, description, annotations and pool/catalog publication. The call must supply that same fingerprint. The binary rechecks endpoint, exposure, capability schema, caller authorization and exact pool/catalog publication before dispatch, rejects runtime or configuration changes during the call, and records against the configuration captured before verification. Calls have a 10-second deadline and 16 KiB verification response limit and an 8 KiB input limit. A completed non-error result records owner-isolated first-use evidence; errors, unsupported task interaction and oversized results do not. Verification output is a status and byte count; returned tool content is not persisted in readiness or rendered by this flow.

Depot projects this optional contract from the authorized, published MCP artifact current revision’s `metadata.mcpConnection`. The metadata must contain the exact revision ID; publication and immutable revision storage remain Depot-owned. The projection is a publisher declaration, not a security approval of the endpoint or its tools. Existing catalog data must explicitly supply valid metadata before listings can use the fast path. Entries without supported metadata retain Add to Library and show the remaining Gateway connection step. Stdio installation, OAuth authorization, nested input forms, and unsupported schema assertions require further versions and qualification.

The supported publisher path is the existing authenticated `depot.skills.ingest_mcp_registry` operation. A server manifest may explicitly declare `_meta["ai.dinglebear.labby/mcp-connection"]` with these same five fields, using `schemaVersion: "labby.mcp-connection-manifest/v1"` and `revisionId: "current"`. The canonical manifest bytes are a revision component; the ingester derives the actual revision ID and binds the public connection metadata to it. Malformed declarations fail ingestion, and importing does not publish the artifact. A publisher separately approves it through `depot.artifacts.set_publication` with the current `expectedVersion`. An approved refresh without the declaration removes the connection projection.

Registry artifacts retain their stored `mcp` kind and stable artifact IDs. Discovery treats `mcp` and `mcp-server` as the same family when filtering and validating results; other kinds remain distinct. Alias handling never rewrites an artifact's identity or grants publication or access.
