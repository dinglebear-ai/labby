---
title: Depot control-plane compatibility contract
status: active
---

# Depot control-plane compatibility contract

`apps/gateway-admin` is the only Labby and Depot frontend. The browser calls
relative Labby URLs; only Labby holds a Depot credential. Depot remains the
authority for Artifact visibility, mutation policy, immutable revisions, and
audit truth.

The checked manifest at
[`fixtures/depot-control-plane/compatibility-v1.json`](fixtures/depot-control-plane/compatibility-v1.json)
is the release denominator. A UI action is available only when its required
operation and contract fingerprint are present. Missing or unknown required
contracts render `incompatible`; they never fall back to a generic operation
console.

## Actor and mount policy

- OAuth browser identity plus a durable Labby principal is required for Depot
  mutation and import routes.
- `none`, `web_ui_auth_disabled`, synthetic development identity, and the
  static-bearer browser shell do not establish a Depot actor.
- A shared Depot service credential is read-only unless the manifest explicitly
  records an approved service-actor policy. Multi-user mutation requires Depot
  delegated actor enforcement.
- Effective permission is the intersection of current Labby permission,
  configured connection ACL, Depot delegated scope, and Depot resource policy.

## Authority epoch

Labby issues an opaque epoch covering its browser-session generation, the
configured Depot connection generation, Depot deployment/account/tenant/team
and principal, the Depot operation fingerprint, and the selected local
destination generation. Cursors, jobs, uploads, intents, confirmations, cache
entries, and receipts are invalid outside that epoch.

## Operational slice

The first release contains only:

1. session and compatibility bootstrap;
2. bounded Artifact list and bounded current-revision detail;
3. operator-configured connection status.

Exact import, create/ingest, lifecycle management, jobs/uploads, and browser
credential administration are expansion capabilities. Each is absent until its
manifest entry is `supported` and its required operation schemas pass drift
checks.

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
