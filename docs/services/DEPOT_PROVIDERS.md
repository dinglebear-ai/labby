---
title: Depot providers
created: "2026-09-04"
updated: "2026-09-30"
status: active
---

# Depot providers

Labby can combine the Public Depot with operator-configured Depot deployments.
The browser uses the versioned routes below `/v1/depot`; endpoints and bearer
credentials remain on the Labby host.

Discovery requires a durable browser session and current `lab:read` permission.
Each result carries both `providerId` and the provider's raw `artifactId`.
Partial provider failure returns successful data with explicit coverage state.
An opaque cursor expires after inactivity, restart, authority change, provider
replacement, or catalog change; clients restart the same query when that occurs.

Provider configuration is browser-only and requires current `lab:admin`
permission. Unsafe provider operations also require the browser session's CSRF
token and the configured canonical Origin. Credential replacement, clearing,
endpoint changes involving credentials, and removal require a fresh one-action
reauthentication proof. Enabling a custom bearer provider grants eligible users
of this Labby instance read discovery through that shared credential.

Disabling a provider is an offline local operation. Removal deletes only the
active credential owned by Labby. It does not revoke the credential at the
provider and recovery snapshots may retain it for the configured retention
period.


## Client work bounds

Host-managed HTTP providers use their validated loopback endpoint without loading
TLS trust roots. HTTPS providers retain native trust validation and DNS pinning;
cold trust setup runs on bounded blocking workers and is coalesced by the existing
one-minute connection lease. A request deadline does not release the blocking
setup slot until its worker finishes.

The Library retains at most 1,000 metadata records, 20 pages, and 8 MiB of compact
JSON projections. It renders up to 200 records per loaded-result page. Continuing
through a large catalog discards older pages with an explicit notice; Refresh
starts again. Filters, counts, and metadata exports operate on retained records.
An export is marked incomplete whenever older results were discarded, even after
the last server cursor was consumed. Artifact inspection uses a separate fetch
and remains available through its deep link.
