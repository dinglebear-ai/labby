# Stash Design

Date: 2026-04-24
Status: Draft approved in brainstorming

## Summary

`Stash` is a new first-class, always-on capability service for managing user-authored agent artifacts in `lab`.

It sits between local authoring and target deployment. Users import or create a component locally, save immutable revisions, sync those revisions through pluggable providers, browse and fetch them on other machines, and export them into existing deploy flows.

`Stash` is:
- component-first
- local-first
- revisioned
- provider-pluggable
- deploy-adjacent, not deploy-owning

V1 first-class component kinds:
- `skill`
- `agent`
- `mcp_server`

V1 sharing model:
- single-user
- multi-machine

## Goals

- Make authored agent components first-class assets inside `lab`.
- Provide a stable library for viewing, versioning, and managing agent artifacts.
- Preserve local authoring as the source of truth.
- Support explicit sync through pluggable providers from day one.
- Support import/adoption from existing marketplace-local content in v1.
- Hand components off cleanly to existing deploy flows without taking over deploy orchestration.

## Non-Goals

- Team collaboration or named multi-user ACLs.
- Public publishing.
- Background sync or hidden reconciliation.
- Three-way merge of diverged component files.
- Provider-to-provider copy orchestration.
- Replacing existing deploy flows.
- Generic cloud-drive file management outside supported component types.

## Product Boundary

`Stash` is the canonical library for user-authored agent components.

It owns:
- component identity and metadata
- local editable workspaces for managed components
- immutable saved revisions
- artifact manifests and bounded previews
- provider linkage and sync state
- cross-machine fetch and export flows

It does not own:
- target-specific deployment execution
- plugin installation and uninstall
- marketplace source discovery
- arbitrary file storage unrelated to supported component kinds

## Architectural Position

`Stash` should be implemented as an always-on product-local capability service, similar in posture to `marketplace` and `extract`.

Recommended ownership split:
- `crates/lab-apis`
  - pure `stash` types only
- `crates/lab/src/dispatch/stash/`
  - shared dispatch logic
  - local store/index management
  - provider adapters
  - import/export logic
- `crates/lab/src/cli/stash.rs`
  - CLI shim
- `crates/lab/src/mcp/services/stash.rs`
  - MCP shim
- `crates/lab/src/api/services/stash.rs`
  - HTTP shim

`stash` should expose one first-class MCP tool using the standard `action` + `params` contract.

## Core Model

### `StashComponent`

Represents a managed authored component.

Fields:
- stable component id
- `kind`: `skill` | `agent` | `mcp_server`
- slug/name
- title
- description
- source workspace root
- primary manifest path
- tags
- origin metadata
- created timestamp
- updated timestamp
- current local revision pointer

### `StashRevision`

Represents an immutable snapshot of a component at save time.

Fields:
- revision id
- component id
- content digest
- manifest summary
- file manifest
- optional save message
- created timestamp
- provider sync status by backend

### `StashArtifact`

Represents an individual file inside a saved revision.

Fields:
- relative path
- content hash
- language/type
- size
- optional bounded inline preview content

### `StashWorkspace`

Represents the editable local mirror for a component.

Fields:
- component id
- workspace root
- base revision id
- dirty state
- last scanned timestamp

### `StashProviderRecord`

Tracks the mapping between local state and a provider.

Fields:
- component id
- provider name
- provider component reference
- provider revision references
- sync cursor or remote head marker
- last push timestamp
- last pull timestamp
- conflict state

## Revision Semantics

`Stash` versions snapshots, not live directories.

Rules:
- local workspaces remain mutable
- `component.save` creates an immutable revision
- revisions must remain readable even if the workspace later changes or is removed
- a revision is the stable unit for cross-machine fetch, sync, and export

This model keeps local editing simple while making remote fetch and deploy handoff deterministic.

## Import and Adopt

V1 includes import from marketplace-local content.

Recommended primary import action:
- `component.import`

Supported sources:
- local filesystem path
- marketplace plugin id, resolved through existing local marketplace workspace or installed payload

Recommended import behavior:
- detect or validate component kind
- materialize into a `Stash`-owned workspace
- record origin metadata
- start `Stash` revision lifecycle from the imported component

Origin metadata fields:
- `origin.kind`: `local_path` | `marketplace_plugin`
- `origin.ref`: source path or `name@marketplace`
- `imported_at`

Recommended v1 materialization mode:
- `copy`

Deferred:
- reference semantics
- live coupling back to marketplace after import

## Relationship to Marketplace

`marketplace` and `stash` share concepts but own different lifecycles.

`marketplace` owns:
- external/plugin-oriented source discovery
- plugin install state
- plugin component inspection
- plugin workspace mirrors around plugin lifecycle

`stash` owns:
- authored component identity
- immutable revision history
- provider sync
- cross-machine fetch
- export/handoff to deployment

Important distinction:
- `marketplace.plugin.workspace` is an editable working copy in a plugin-oriented lifecycle
- `stash.component.workspace` is the canonical authored workspace for a managed component

V1 should support import from marketplace-local content, but imported components become `Stash`-owned artifacts with no implicit live sync back to marketplace.

## Provider Architecture

`Stash` is local-first. The local stash store is canonical.

Providers are adapters that sync revisions to and from external backing stores.

### Canonical local responsibilities

The local store owns:
- component metadata index
- immutable revision metadata
- local artifact/object storage
- workspace mapping
- provider linkage state

### Provider responsibilities

A provider adapter should be able to:
- report identity and capabilities
- link a component to remote storage
- push a revision
- list remote revisions for a linked component
- pull a revision
- fetch individual remote artifacts when needed
- report sync status

### Recommended provider contract

- `name()`
- `kind()`
- `capabilities()`
- `link_component(component)`
- `push_revision(component, revision)`
- `list_remote_revisions(component)`
- `pull_revision(component, remote_ref)`
- `fetch_artifact(remote_ref, path)`
- `sync_status(component)`

### Recommended v1 provider capability flags

- `push_revision`
- `pull_revision`
- `list_remote`
- `content_addressed`
- `supports_metadata`
- `supports_preview_urls`

### First providers

V1 should ship with:
- `filesystem`
- `google_drive`

Rationale:
- `filesystem` provides the simplest portable baseline and test target
- `google_drive` matches the intended UX without contaminating the core model with Drive-specific semantics

## Sync Model

Sync must be explicit in v1.

Rules:
- no background reconciliation
- no hidden automatic push or pull
- callers explicitly invoke provider sync actions
- divergence is surfaced structurally
- force is explicit when overwrite is allowed

Conflict rule for v1:
- if remote state diverges from linked local head, mark the component as conflicted
- block blind overwrite unless forced
- do not attempt three-way merge in v1

## Local Storage Layout

Recommended managed root under lab state:

- `components/`
  - component metadata records
- `revisions/`
  - immutable revision manifests
- `objects/`
  - content-addressed blobs or packed revision payloads
- `workspaces/`
  - editable materialized component roots
- `providers/`
  - linkage and sync state

Key invariant:
- saved revisions must remain valid independent of workspace state

## Public Surface

### MCP

One tool:
- `stash({ action, params })`

Recommended v1 actions:
- `components.list`
- `component.get`
- `component.import`
- `component.workspace`
- `component.save`
- `component.revisions`
- `component.fetch`
- `component.export`
- `providers.list`
- `provider.link`
- `provider.sync.status`
- `provider.push`
- `provider.pull`

### CLI

Recommended command shape:
- `lab stash components list`
- `lab stash component get <id>`
- `lab stash component import --path <path>`
- `lab stash component import --plugin <name@marketplace>`
- `lab stash component save <id> --message ...`
- `lab stash component revisions <id>`
- `lab stash provider push <id> --provider <name>`
- `lab stash provider pull <id> --provider <name>`
- `lab stash component export <id> --revision <rev>`

### HTTP

Recommended route group:
- `/v1/stash/*`

HTTP should mirror the shared dispatch operations rather than inventing a separate model.

## Primary User Flows

### 1. Import a component

- user imports from local path or marketplace plugin id
- `Stash` validates kind and layout
- `Stash` copies content into a managed workspace
- `Stash` records metadata and origin

### 2. Save a revision

- user invokes `component.save`
- workspace is snapshotted into an immutable revision
- file manifest and digest are computed
- revision becomes available for preview, sync, fetch, and export

### 3. Push to a provider

- user invokes `provider.push`
- selected revision is uploaded to the linked provider
- provider references and sync timestamps are recorded only after success

### 4. Pull on another machine

- user invokes `provider.pull` or `component.fetch`
- revision is fetched into the local stash store
- caller may materialize or update a local workspace
- no automatic deploy occurs

### 5. Export to deploy flow

- user invokes `component.export`
- `Stash` emits a handoff-ready directory or bundle
- existing deployment flow remains the owner of target writes

## Error Model

`stash` should use the shared error taxonomy from `docs/ERRORS.md` wherever possible.

Expected baseline kinds:
- `unknown_action`
- `missing_param`
- `invalid_param`
- `not_found`
- `validation_failed`
- `internal_error`
- `server_error`

Expected stash-specific additions or usages:
- `conflict`
- `unsupported_provider`
- `unsupported_component_kind`
- `integrity_mismatch`
- `sync_failed`

Behavior rules:
- save failures must not expose partial visible revisions
- provider state updates only after confirmed remote success
- import failure on invalid component layout returns `validation_failed`
- import failure on unknown path or plugin returns `not_found`

## Observability and Safety

When implemented, `stash` must follow the same observability rules as other first-class services.

Required principles:
- dispatch events for every user-visible action
- explicit logging for save, import, push, pull, and export outcomes
- destructive or overwrite-like operations must log intent and result
- provider secrets or tokens must never appear in logs

## Verification Targets

V1 verification should cover:
- import a local `skill`, `agent`, and `mcp_server`
- import from marketplace-local content and preserve origin metadata
- save multiple revisions and confirm immutability
- push and pull through `filesystem`
- push and pull through `google_drive`
- export a selected revision into an existing deploy handoff path
- verify bounded preview and artifact listing behavior remains MCP-safe
- verify conflicts are surfaced cleanly and do not auto-merge

## Open Follow-Up Questions For Implementation Planning

- Whether `component.fetch` is distinct from `provider.pull` or a convenience alias.
- Whether revisions are stored as packed bundles, per-file blobs, or a hybrid object store.
- Whether `google_drive` stores each revision as a structured folder, archive object, or manifest plus blobs.
- Whether export emits a directory tree only or also supports a bundled archive format.
- Whether component kind detection should be strict or allow manual override in all import paths.

## Decision Summary

Chosen design decisions:
- component-first model
- v1 kinds limited to `skill`, `agent`, `mcp_server`
- local-first canonical model
- pluggable providers from day one
- library plus deploy handoff, not deploy orchestration
- single-user multi-machine sharing boundary
- import from marketplace-local content in v1
- explicit sync only
- no live marketplace coupling after import
