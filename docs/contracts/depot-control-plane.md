---
title: Depot control-plane compatibility contract
status: active
created: 2026-09-03
updated: 2026-09-06
---

# Depot control-plane compatibility contract

`apps/gateway-admin` is the only Labby and Depot frontend. The browser calls
relative Labby URLs; only Labby holds a Depot credential. Depot remains the
authority for Artifact visibility, mutation policy, immutable revisions, and
audit truth.

The checked federated manifest at
[`fixtures/depot-control-plane/compatibility-v2.json`](fixtures/depot-control-plane/compatibility-v2.json)
is the release denominator. A UI action is available only when its required
operation and contract fingerprint are present. Administration renders Depot's
published input schemas as typed controls. Missing or unknown required contracts
render `incompatible`; Labby never invents an unadvertised operation.

## Actor and mount policy

- OAuth browser identity plus a durable Labby principal is required for Depot
  mutation and import routes.
- `none`, `web_ui_auth_disabled`, synthetic development identity, and the
  static-bearer browser shell do not establish a Depot actor.
- A shared Depot service credential may mutate only when the browser principal
  currently holds `lab:admin`, the request carries valid session CSRF, and the
  credential itself carries Depot's required write authority. Depot remains the
  final scope and resource-policy authority.
- Effective permission is the intersection of current Labby permission,
  configured connection ACL, Depot delegated scope, and Depot resource policy.

## Authority epoch

Labby issues an opaque epoch covering its browser-session generation, the
configured Depot connection generation, Depot deployment/account/tenant/team
and principal, the Depot operation fingerprint, and the selected local
destination generation. Cursors, jobs, uploads, intents, confirmations, cache
entries, and receipts are invalid outside that epoch.

## Operational surface

The Administration surface consumes Depot's authorization-filtered canonical
operation catalog. It covers Artifact and Skill lifecycle, sources, ingestion,
uploads, bundles, token administration, and privileged maintenance. Labby keeps
provider connection management beside those operations while Depot remains the
authority for the operation schemas, visibility, revisions, and execution.

Discovery's **Send to Labby** action resolves the selected provider to an
Artifact acquisition connection with the same ID, requests the exact selected
Artifact revision through Depot's `/api/artifacts/exact` contract, verifies its
components, and commits the result through Labby's `artifacts.import` action.
It fails closed when the matching acquisition connection or exact revision is
missing; it never substitutes another configured Depot.

## Bounded transport

- Artifact pages contain at most 200 summaries. Continuation cursors are opaque,
  visibility-bound, and listing-generation-bound.
- Detail contains one current revision and a revision count; unbounded history
  is never returned in the detail envelope.
- Exact export returns `dinglebear.artifact-interchange/v1` and relative
  same-origin component locators. Components are authenticated independently,
  digest verified, and subject to Labby's existing file/package limits.
- Authority responses use `Cache-Control: private, no-store`. Redirects,
  alternate origins, HTML fallthrough, and unbounded decompression are errors.

Federated discovery uses provider-qualified identities and a random 256-bit
Labby cursor. It fairly merges at most one bounded page from each provider,
keeps upstream continuations server-side, reports pending and failed coverage
separately, and expires backscroll after two replayable transitions. Artifact
detail always requires the exact pair of provider ID and raw artifact ID.

## Retry and result truth

Reads may retry after a successful session refresh if the authority epoch is
unchanged. Mutations never replay blindly. A supported mutation carries one
server-bound intent key; an ambiguous response remains `indeterminate` until
the same intent is reconciled. Browser disconnect is not proof of remote
cancellation.

## Compatibility evidence

Release evidence records the Labby commit and binary digest, frontend export
manifest digest, Depot commit and signed image digest, this manifest digest,
operation fingerprint, auth/actor mode, and durable schema generation. Source
checkout combinations are not authoritative release evidence.
