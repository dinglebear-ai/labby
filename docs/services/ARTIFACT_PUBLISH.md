---
title: Artifact publishing
created: "2026-09-20"
updated: "2026-09-20"
status: active
---

# Artifact publishing

`artifact_publish` is Labby's protected-route service for publishing a bounded
Skill archive to the authenticated Team's Artifact authority. Call
`artifacts.publish_skill_archive` with `filename`, base64-encoded
`archive_base64`, and an optional `namespace`.

The service appears only on an eligible protected Team route. It requires a
current project grant with Artifact publication authority and the route's
admitted `team-depot` backend. The backend connection is server-side direct
HTTPS; callers do not configure or call a Depot MCP tool.

Publication uses one shared three-phase workflow:

1. create an authorized upload slot;
2. transfer the bounded archive bytes; and
3. start archive ingestion.

Labby revalidates the route, project grant, target binding, and publication
authority before each phase. Revocation or a changed binding stops the workflow
before the next external request. Errors identify the failed stage through the
shared typed and redacted error envelope. Credentials, delegations, and raw
backend responses are never returned.

Success returns an `IngestJobReceipt`. A queued or running ingestion job means
the archive was accepted for processing; it does not prove that a public
listing is live. Observe the returned job through Labby's `jobs` service until
it reaches a terminal state, then read the resulting Artifact publication.

## Compatibility

The former `depot_publish` service and `depot.publish_skill_archive` action are
accepted as a deprecated compatibility pair. They invoke the same validation,
authorization, transport, and workflow as the canonical contract. The legacy
service is hidden from new MCP discovery and must not be added to new Loadouts
or integrations.
