---
title: "Dev Containers"
created: "2026-09-07"
updated: "2026-09-13"
---

# Dev Containers

Dev Containers are owner-scoped, quota-bounded development environments. This
document freezes the contract and persistence boundary. Labby registers the
`dev_containers` service with the actions `dev_containers.list`, `create`,
`start`, `stop`, `destroy`, and `reconcile`, exposed over HTTP at
`POST /v1/dev-containers` and as the `dev_containers` MCP tool. The container
engine is the pluggable `labby_runtime::dev_container_runtime::ContainerRuntime`
contract. Production uses an explicitly configured, project-restricted Incus
HTTPS endpoint. Launches fail closed when that endpoint or any required mutual
TLS credential is absent or invalid; Labby never falls back to a local Incus
socket or the Incus `default` project.

Every instance has exactly one installation, Team, Project, or Personal owner.
Its durable record pins an administrator-approved template and an immutable
`sha256:<64 lowercase hex>` image digest. Tags and caller-supplied image names
are never launch authority.

## Admission

A launch is admitted only when all of these remain true at the final execution
boundary:

- the caller currently has the required capability over the exact owner scope;
- the template is still approved and its pinned image digest is unchanged;
- the owner's active-instance quota has capacity;
- requested CPU, memory, disk, and lifetime are non-zero and do not exceed the
  template ceiling; and
- every requested host capability is explicitly approved by the template.

Host access is default-denied. Privileged execution, the host filesystem,
container-runtime sockets, host networking, host devices, and kernel
administration are separate capabilities. Approval of one never implies
another.

Secrets are stored as opaque secret references. Secret values, environment
material, credentials, and decrypted content do not belong in the instance
ledger, audit records, API payloads, or logs.

## Durable lifecycle

The ledger stores the typed owner, instance ID, template ID, image digest,
lifecycle nonce, desired state, observed state, quota reservation, secret
references, authority epoch/fingerprint, revision, and timestamps. Desired
states are `running`, `stopped`, and `deleted`; observed states are `pending`,
`starting`, `running`, `stopping`, `stopped`, `failed`, and `deleted`.

Every create or recreate receives a new unpredictable lifecycle nonce. Runtime
observations and cleanup receipts must carry that exact nonce, so a late event
from an earlier instance cannot mutate or delete a replacement that reused the
same external name. Desired/observed transitions use compare-and-swap over the
ledger revision.

Deletion is terminal for a lifecycle nonce. Durable state is retained long
enough to reconcile cleanup and prove quota release; callers cannot restore a
deleted nonce. A new instance is a new lifecycle.

## Revocation and failure

Membership, policy, template, credential, or owner changes invalidate retained
authority at the next safe boundary. Admission, runtime start, credential
checkout, external effects, observation commit, stop, deletion, and retained
resume reauthorize current state. Missing, corrupt, stale, or mismatched state
fails closed.

An indeterminate runtime outcome remains reconciliable state; it is not reported
as success and its quota reservation is not silently released. Cleanup acts
only on resources proven to carry the ledger's instance ID and lifecycle nonce.

This contract does not authorize direct access to a container engine. The HTTP
and MCP adapters are thin: they pass the `action` plus `params` envelope to the
shared `dev_containers` dispatch, which owns admission, ledger, and
reconciliation semantics. Exact parameters, scopes, and destructive
classification are in the generated [action catalog](../generated/action-catalog.md).

## Incus runtime

The runtime certificate must be restricted by Incus to one dedicated project.
That project must be confined to the approved managed network and storage pool,
deny privileged and nested containers, deny low-level configuration and host
devices, and enforce project-wide instance, CPU, memory, disk, and process
limits. Its default profile supplies only the root disk and a managed NIC. It
must not contain host paths or runtime sockets.

Labby derives each Incus instance name from the durable instance ID plus its
lifecycle nonce, labels the instance with both values, and verifies those
labels before every state change or deletion. Creation sends only the approved
image fingerprint and the stored CPU, memory, disk, and lifetime ceilings.
Templates that request a host capability are rejected by this runtime.

The required environment variables and credential-file rules are documented in
[Environment Variables](../runtime/ENV.md#dev-container-runtime).
