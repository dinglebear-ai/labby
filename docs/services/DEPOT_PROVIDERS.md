---
title: Depot providers
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
