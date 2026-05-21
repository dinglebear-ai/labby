# Servarr Shared-Primitives Extraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract the 11 genuinely-shared *arr primitive types out of `radarr/types/*` into `lab_apis::servarr::types::*`, and replace the radarr copies (plus the matching sonarr/prowlarr stub files) with one-line re-exports so every *arr client sees the same shapes.

**Architecture:** `servarr` is a types-only, client-less module — no HTTP, no endpoints, just data structures shared across radarr/sonarr/prowlarr. Each *arr service keeps its own `types/<name>.rs` file so its module tree stays uniform, but the file becomes a `pub use crate::servarr::types::<name>::*;` shim. Types that compose with service-specific ids (history → `MovieId`, calendar → `Movie`, import_list → `TmdbId`, `ManualImportItem` → `MovieId`, `TagDetail` → `movie_ids`) stay per-service and are NOT touched by this plan.

**Tech Stack:** Rust 2024, serde, feature-gated modules, `cargo check -p lab-apis` as the verification loop.

---

## Scope

**In scope — 11 full extractions (radarr owns the source content today):**
1. `quality` — `QualityProfileId`, `QualityProfile`, `QualityDefinition`
2. `command` — `CommandId`, `CommandStatus`, `Command`
3. `download_client` — `DownloadClientId`, `RemotePathMappingId`, `DownloadClient`, `RemotePathMapping`
4. `notification` — `NotificationId`, `Notification`
5. `metadata` — `MetadataId`, `Metadata`
6. `system` — `SystemStatus`, `HealthSeverity`, `HealthCheck`, `LogFile`, `UpdateInfo`, `DiskSpace`
7. `indexer` — `IndexerId`, `Indexer`
8. `language` — `LanguageId`, `Language`
9. `auth` — `LoginRequest`
10. `release` — `Release`
11. `root_folder` — `RootFolderId`, `RootFolder`

**Partial — `filesystem`:** `FilesystemEntryType`, `FilesystemEntry`, `FilesystemListing` move to servarr. `ManualImportItem` (references `MovieId`) stays in radarr.

**Partial — `tag`:** `TagId`, `Tag` move to servarr. `TagDetail` (radarr-specific field names like `movie_ids`) stays in radarr.

**Out of scope — per-service, do NOT touch:** `radarr/types/history.rs`, `radarr/types/calendar.rs`, `radarr/types/import_list.rs`, and their sonarr/prowlarr equivalents.

**Prowlarr caveat:** prowlarr's stub files use plural names (`indexers.rs`, `tags.rs`, `notifications.rs`, `download_clients.rs`) that don't match servarr singular naming. This plan keeps the plural filenames and puts the re-export inside them — no renaming, no `prowlarr/types.rs` edits.

## File Structure

**Created:**
- `crates/lab-apis/src/servarr.rs` — module declaration + `META` const
- `crates/lab-apis/src/servarr/types.rs` — re-exports every sub-module flat
- `crates/lab-apis/src/servarr/error.rs` — `ServarrError` (shared error shell)
- `crates/lab-apis/src/servarr/types/quality.rs` — type definitions
- `crates/lab-apis/src/servarr/types/command.rs` — type definitions
- `crates/lab-apis/src/servarr/types/download_client.rs` — type definitions
- `crates/lab-apis/src/servarr/types/notification.rs` — type definitions
- `crates/lab-apis/src/servarr/types/metadata.rs` — type definitions
- `crates/lab-apis/src/servarr/types/system.rs` — type definitions
- `crates/lab-apis/src/servarr/types/tag.rs` — type definitions (`Tag` + `TagId` only)
- `crates/lab-apis/src/servarr/types/indexer.rs` — type definitions
- `crates/lab-apis/src/servarr/types/language.rs` — type definitions
- `crates/lab-apis/src/servarr/types/auth.rs` — type definitions
- `crates/lab-apis/src/servarr/types/release.rs` — type definitions
- `crates/lab-apis/src/servarr/types/root_folder.rs` — type definitions
- `crates/lab-apis/src/servarr/types/filesystem.rs` — type definitions (entry + listing only)

**Modified (overwritten with re-exports):**
- `crates/lab-apis/src/radarr/types/{quality,command,download_client,notification,metadata,system,indexer,language,auth,release,root_folder}.rs` — 11 files become one-liner re-exports
- `crates/lab-apis/src/radarr/types/tag.rs` — re-export + keep `TagDetail`
- `crates/lab-apis/src/radarr/types/filesystem.rs` — re-export + keep `ManualImportItem`
- `crates/lab-apis/src/sonarr/types/{quality,command,download_client,notification,metadata,system,indexer,language,auth,release,root_folder,tag,filesystem}.rs` — empty → one-liner re-exports (13 files; sonarr uses singular names)
- `crates/lab-apis/src/prowlarr/types/{language,system,filesystem}.rs` — the three singular-named files prowlarr has that match servarr
- `crates/lab-apis/src/prowlarr/types/{indexers,notifications,tags,download_clients}.rs` — plural filenames, re-export from singular servarr module
- `crates/lab-apis/Cargo.toml` — `radarr`, `sonarr`, `prowlarr` features gain `"servarr"` as a dependency

---

### Task 1: Create servarr module root + error shell

**Files:**
- Create: `crates/lab-apis/src/servarr.rs`
- Create: `crates/lab-apis/src/servarr/error.rs`

- [ ] **Step 1: Write `servarr.rs`**

```rust
//! Shared primitives for the *arr family (Radarr, Sonarr, Prowlarr, …).
//!
//! Types here have identical shape across every *arr service. Service-specific
//! types (e.g. history records that reference `MovieId` or `SeriesId`) stay
//! in the per-service module.
//!
//! This module is types-only — no HTTP client, no endpoints, no `PluginMeta`.
//! Each *arr service re-exports these shapes from its own `types/*.rs` files
//! so downstream callers can keep writing `radarr::types::Quality` without
//! caring where the definition lives.

pub mod error;
pub mod types;

pub use error::ServarrError;
```

- [ ] **Step 2: Write `servarr/error.rs`**

```rust
//! Shared error type for the *arr family.
//!
//! Per-service errors (`RadarrError`, `SonarrError`, …) can wrap this via
//! `#[from]` when needed. Kept deliberately minimal — the real HTTP/JSON
//! plumbing lives in each service's client, not here.

use thiserror::Error;

/// Errors shared across *arr services.
#[derive(Debug, Error)]
pub enum ServarrError {
    /// A resource lookup returned 404.
    #[error("not found")]
    NotFound,
}
```

- [ ] **Step 3: Verify it compiles with the servarr feature**

Run: `cargo check -p lab-apis --features servarr`
Expected: clean build. If it fails with "file not found for module `types`", that's correct — Task 2 creates it.

- [ ] **Step 4: Commit**

```bash
git add crates/lab-apis/src/servarr.rs crates/lab-apis/src/servarr/error.rs
git commit -m "feat(servarr): add module root and error shell"
```

---

### Task 2: Create servarr/types.rs aggregator

**Files:**
- Create: `crates/lab-apis/src/servarr/types.rs`

- [ ] **Step 1: Write the aggregator**

```rust
//! Shared *arr types, one sub-module per resource.
//!
//! Each sub-module mirrors the matching `*Resource` shape from the Radarr /
//! Sonarr / Prowlarr OpenAPI specs. Only the fields that are identical across
//! services live here; service-specific fields stay in the per-service
//! `types/<name>.rs` file alongside a `pub use` of these shapes.

pub mod auth;
pub mod command;
pub mod download_client;
pub mod filesystem;
pub mod indexer;
pub mod language;
pub mod metadata;
pub mod notification;
pub mod quality;
pub mod release;
pub mod root_folder;
pub mod system;
pub mod tag;
```

- [ ] **Step 2: Verify**

Run: `cargo check -p lab-apis --features servarr`
Expected: one error per missing sub-module file. That's fine — Tasks 3–15 create them.

- [ ] **Step 3: Commit**

```bash
git add crates/lab-apis/src/servarr/types.rs
git commit -m "feat(servarr): declare type sub-modules"
```

---

### Task 3: Extract `quality` types

**Files:**
- Create: `crates/lab-apis/src/servarr/types/quality.rs`
- Modify: `crates/lab-apis/src/radarr/types/quality.rs`
- Modify: `crates/lab-apis/src/sonarr/types/quality.rs`

- [ ] **Step 1: Write `servarr/types/quality.rs`**

```rust
//! Quality profile and definition types.

use serde::{Deserialize, Serialize};

/// Newtype wrapper around a quality-profile id.
///
/// Distinct from indexer / movie / command ids so the type system rejects
/// cross-wiring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct QualityProfileId(pub i64);

/// A quality profile — the set of qualities an *arr service is willing to
/// accept, plus the upgrade rules between them.
///
/// Mirrors `QualityProfileResource` from the Radarr v3 / Sonarr v3 OpenAPI specs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityProfile {
    pub id: QualityProfileId,
    pub name: String,
    pub upgrade_allowed: bool,
    pub cutoff: i32,
    #[serde(default)]
    pub items: serde_json::Value,
}

/// A quality definition — size/megabit rules per quality level.
///
/// Mirrors `QualityDefinitionResource` from the Radarr v3 / Sonarr v3 OpenAPI specs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityDefinition {
    pub id: i64,
    pub title: String,
    pub weight: i32,
    pub min_size: Option<f64>,
    pub max_size: Option<f64>,
    pub preferred_size: Option<f64>,
}
```

- [ ] **Step 2: Replace `radarr/types/quality.rs` with re-export**

```rust
//! Quality profile and definition types (re-exported from servarr).

pub use crate::servarr::types::quality::*;
```

- [ ] **Step 3: Replace `sonarr/types/quality.rs` with re-export**

```rust
//! Quality profile and definition types (re-exported from servarr).

pub use crate::servarr::types::quality::*;
```

- [ ] **Step 4: Verify**

Run: `cargo check -p lab-apis --features "radarr sonarr servarr"`
Expected: clean build for the `quality` module; other missing modules still error — that's fine.

- [ ] **Step 5: Commit**

```bash
git add crates/lab-apis/src/servarr/types/quality.rs crates/lab-apis/src/radarr/types/quality.rs crates/lab-apis/src/sonarr/types/quality.rs
git commit -m "refactor(servarr): extract quality types"
```

---

### Task 4: Extract `command` types

**Files:**
- Create: `crates/lab-apis/src/servarr/types/command.rs`
- Modify: `crates/lab-apis/src/radarr/types/command.rs`
- Modify: `crates/lab-apis/src/sonarr/types/command.rs`

- [ ] **Step 1: Write `servarr/types/command.rs`**

```rust
//! Command (async job) types.

use serde::{Deserialize, Serialize};

/// Newtype wrapper around a command id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CommandId(pub i64);

/// Lifecycle status for an async command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CommandStatus {
    Queued,
    Started,
    Completed,
    Failed,
    Aborted,
}

/// A queued / running / completed *arr command.
///
/// Mirrors `CommandResource` from the Radarr v3 / Sonarr v3 OpenAPI specs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Command {
    pub id: CommandId,
    pub name: String,
    pub status: CommandStatus,
    #[serde(default)]
    pub queued: Option<String>,
    #[serde(default)]
    pub started: Option<String>,
    #[serde(default)]
    pub ended: Option<String>,
    #[serde(default)]
    pub duration: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}
```

- [ ] **Step 2: Replace `radarr/types/command.rs`**

```rust
//! Command types (re-exported from servarr).

pub use crate::servarr::types::command::*;
```

- [ ] **Step 3: Replace `sonarr/types/command.rs`**

```rust
//! Command types (re-exported from servarr).

pub use crate::servarr::types::command::*;
```

- [ ] **Step 4: Verify**

Run: `cargo check -p lab-apis --features "radarr sonarr servarr"`
Expected: `command` module clean.

- [ ] **Step 5: Commit**

```bash
git add crates/lab-apis/src/servarr/types/command.rs crates/lab-apis/src/radarr/types/command.rs crates/lab-apis/src/sonarr/types/command.rs
git commit -m "refactor(servarr): extract command types"
```

---

### Task 5: Extract `download_client` types

**Files:**
- Create: `crates/lab-apis/src/servarr/types/download_client.rs`
- Modify: `crates/lab-apis/src/radarr/types/download_client.rs`
- Modify: `crates/lab-apis/src/sonarr/types/download_client.rs`
- Modify: `crates/lab-apis/src/prowlarr/types/download_clients.rs` (plural name)

- [ ] **Step 1: Write `servarr/types/download_client.rs`**

```rust
//! Download client and remote-path-mapping types.

use serde::{Deserialize, Serialize};

/// Newtype wrapper around a download-client id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DownloadClientId(pub i64);

/// Newtype wrapper around a remote-path-mapping id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RemotePathMappingId(pub i64);

/// A configured download client (SABnzbd, qBittorrent, Deluge, …).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadClient {
    pub id: DownloadClientId,
    pub name: String,
    pub implementation: String,
    pub protocol: String,
    pub enable: bool,
    pub priority: i32,
    #[serde(default)]
    pub tags: Vec<i64>,
    #[serde(default)]
    pub fields: serde_json::Value,
}

/// A remote-path mapping — translates a download-client-visible path to an
/// *arr-visible path when the two run on different hosts or containers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePathMapping {
    pub id: RemotePathMappingId,
    pub host: String,
    pub remote_path: String,
    pub local_path: String,
}
```

- [ ] **Step 2: Replace `radarr/types/download_client.rs`**

```rust
//! Download client types (re-exported from servarr).

pub use crate::servarr::types::download_client::*;
```

- [ ] **Step 3: Replace `sonarr/types/download_client.rs`**

```rust
//! Download client types (re-exported from servarr).

pub use crate::servarr::types::download_client::*;
```

- [ ] **Step 4: Replace `prowlarr/types/download_clients.rs` (plural filename, singular servarr module)**

```rust
//! Download client types (re-exported from servarr).

pub use crate::servarr::types::download_client::*;
```

- [ ] **Step 5: Verify**

Run: `cargo check -p lab-apis --features "radarr sonarr prowlarr servarr"`
Expected: `download_client` module clean.

- [ ] **Step 6: Commit**

```bash
git add crates/lab-apis/src/servarr/types/download_client.rs crates/lab-apis/src/radarr/types/download_client.rs crates/lab-apis/src/sonarr/types/download_client.rs crates/lab-apis/src/prowlarr/types/download_clients.rs
git commit -m "refactor(servarr): extract download_client types"
```

---

### Task 6: Extract `notification` types

**Files:**
- Create: `crates/lab-apis/src/servarr/types/notification.rs`
- Modify: `crates/lab-apis/src/radarr/types/notification.rs`
- Modify: `crates/lab-apis/src/sonarr/types/notification.rs`
- Modify: `crates/lab-apis/src/prowlarr/types/notifications.rs` (plural)

- [ ] **Step 1: Write `servarr/types/notification.rs`**

```rust
//! Notification provider types.

use serde::{Deserialize, Serialize};

/// Newtype wrapper around a notification id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NotificationId(pub i64);

/// A configured notification provider.
///
/// Mirrors `NotificationResource` from the Radarr v3 / Sonarr v3 OpenAPI specs.
/// Radarr-specific event flags (`on_movie_delete`, `on_movie_file_delete`) are
/// kept here because the field exists in the payload across services — it is
/// just unused on non-movie services. Callers that need strict typing should
/// access `fields` instead.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub id: NotificationId,
    pub name: String,
    pub implementation: String,
    #[serde(default)]
    pub on_grab: bool,
    #[serde(default)]
    pub on_download: bool,
    #[serde(default)]
    pub on_upgrade: bool,
    #[serde(default)]
    pub on_rename: bool,
    #[serde(default)]
    pub on_movie_delete: bool,
    #[serde(default)]
    pub on_movie_file_delete: bool,
    #[serde(default)]
    pub on_health_issue: bool,
    #[serde(default)]
    pub tags: Vec<i64>,
    #[serde(default)]
    pub fields: serde_json::Value,
}
```

- [ ] **Step 2: Replace `radarr/types/notification.rs`**

```rust
//! Notification types (re-exported from servarr).

pub use crate::servarr::types::notification::*;
```

- [ ] **Step 3: Replace `sonarr/types/notification.rs`**

```rust
//! Notification types (re-exported from servarr).

pub use crate::servarr::types::notification::*;
```

- [ ] **Step 4: Replace `prowlarr/types/notifications.rs`**

```rust
//! Notification types (re-exported from servarr).

pub use crate::servarr::types::notification::*;
```

- [ ] **Step 5: Verify**

Run: `cargo check -p lab-apis --features "radarr sonarr prowlarr servarr"`
Expected: `notification` module clean.

- [ ] **Step 6: Commit**

```bash
git add crates/lab-apis/src/servarr/types/notification.rs crates/lab-apis/src/radarr/types/notification.rs crates/lab-apis/src/sonarr/types/notification.rs crates/lab-apis/src/prowlarr/types/notifications.rs
git commit -m "refactor(servarr): extract notification types"
```

---

### Task 7: Extract `metadata` types

**Files:**
- Create: `crates/lab-apis/src/servarr/types/metadata.rs`
- Modify: `crates/lab-apis/src/radarr/types/metadata.rs`
- Modify: `crates/lab-apis/src/sonarr/types/metadata.rs`

- [ ] **Step 1: Write `servarr/types/metadata.rs`**

```rust
//! Metadata writer types.

use serde::{Deserialize, Serialize};

/// Newtype wrapper around a metadata-writer id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MetadataId(pub i64);

/// A configured metadata writer (Kodi NFO, Wdtv, Plex matroska, …).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    pub id: MetadataId,
    pub name: String,
    pub implementation: String,
    pub enable: bool,
    #[serde(default)]
    pub tags: Vec<i64>,
    #[serde(default)]
    pub fields: serde_json::Value,
}
```

- [ ] **Step 2: Replace `radarr/types/metadata.rs`**

```rust
//! Metadata types (re-exported from servarr).

pub use crate::servarr::types::metadata::*;
```

- [ ] **Step 3: Replace `sonarr/types/metadata.rs`**

```rust
//! Metadata types (re-exported from servarr).

pub use crate::servarr::types::metadata::*;
```

- [ ] **Step 4: Verify**

Run: `cargo check -p lab-apis --features "radarr sonarr servarr"`
Expected: `metadata` module clean.

- [ ] **Step 5: Commit**

```bash
git add crates/lab-apis/src/servarr/types/metadata.rs crates/lab-apis/src/radarr/types/metadata.rs crates/lab-apis/src/sonarr/types/metadata.rs
git commit -m "refactor(servarr): extract metadata types"
```

---

### Task 8: Extract `system` types

**Files:**
- Create: `crates/lab-apis/src/servarr/types/system.rs`
- Modify: `crates/lab-apis/src/radarr/types/system.rs`
- Modify: `crates/lab-apis/src/sonarr/types/system.rs`
- Modify: `crates/lab-apis/src/prowlarr/types/system.rs`

- [ ] **Step 1: Write `servarr/types/system.rs`**

```rust
//! System status, health, log, update, and disk-space types.

use serde::{Deserialize, Serialize};

/// System status — version, runtime, paths. Shape is identical across *arr
/// services; `app_name` disambiguates which product is responding.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemStatus {
    pub version: String,
    pub build_time: String,
    pub is_debug: bool,
    pub is_production: bool,
    pub is_admin: bool,
    pub is_user_interactive: bool,
    pub startup_path: String,
    pub app_data: String,
    pub os_name: String,
    #[serde(default)]
    pub os_version: Option<String>,
    pub is_linux: bool,
    pub is_osx: bool,
    pub is_windows: bool,
    pub is_docker: bool,
    pub mode: String,
    pub branch: String,
    pub runtime_version: String,
    pub runtime_name: String,
    pub app_name: String,
    pub instance_name: Option<String>,
    #[serde(default)]
    pub database_type: Option<String>,
    #[serde(default)]
    pub database_version: Option<String>,
}

/// Severity of a health check finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HealthSeverity {
    Ok,
    Notice,
    Warning,
    Error,
}

/// One health-check entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthCheck {
    pub source: String,
    #[serde(rename = "type")]
    pub severity: HealthSeverity,
    pub message: String,
    #[serde(default)]
    pub wiki_url: Option<String>,
}

/// A log file recorded by an *arr service.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogFile {
    pub id: i64,
    pub filename: String,
    pub last_write_time: String,
    pub content_url: String,
    pub download_url: String,
}

/// One available update.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub branch: String,
    pub release_date: String,
    #[serde(default)]
    pub file_name: Option<String>,
    pub installable: bool,
    pub latest: bool,
    #[serde(default)]
    pub changes: serde_json::Value,
}

/// Disk-space report for a mount point.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskSpace {
    pub path: String,
    pub label: String,
    pub free_space: i64,
    pub total_space: i64,
}
```

- [ ] **Step 2: Replace `radarr/types/system.rs`**

```rust
//! System types (re-exported from servarr).

pub use crate::servarr::types::system::*;
```

- [ ] **Step 3: Replace `sonarr/types/system.rs`**

```rust
//! System types (re-exported from servarr).

pub use crate::servarr::types::system::*;
```

- [ ] **Step 4: Replace `prowlarr/types/system.rs`**

```rust
//! System types (re-exported from servarr).

pub use crate::servarr::types::system::*;
```

- [ ] **Step 5: Verify**

Run: `cargo check -p lab-apis --features "radarr sonarr prowlarr servarr"`
Expected: `system` module clean.

- [ ] **Step 6: Commit**

```bash
git add crates/lab-apis/src/servarr/types/system.rs crates/lab-apis/src/radarr/types/system.rs crates/lab-apis/src/sonarr/types/system.rs crates/lab-apis/src/prowlarr/types/system.rs
git commit -m "refactor(servarr): extract system types"
```

---

### Task 9: Extract `indexer` types

**Files:**
- Create: `crates/lab-apis/src/servarr/types/indexer.rs`
- Modify: `crates/lab-apis/src/radarr/types/indexer.rs`
- Modify: `crates/lab-apis/src/sonarr/types/indexer.rs`
- Modify: `crates/lab-apis/src/prowlarr/types/indexers.rs` (plural)

- [ ] **Step 1: Write `servarr/types/indexer.rs`**

```rust
//! Indexer configuration types.

use serde::{Deserialize, Serialize};

/// Newtype wrapper around an indexer id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IndexerId(pub i64);

/// An indexer configured in an *arr service (typically managed by Prowlarr).
///
/// Mirrors `IndexerResource` from the Radarr v3 / Sonarr v3 / Prowlarr v1
/// OpenAPI specs. Only the display-relevant fields are modeled; the full
/// `fields` array (arbitrary key/value settings) is kept as raw JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Indexer {
    pub id: IndexerId,
    pub name: String,
    pub implementation: String,
    pub protocol: String,
    pub enable: bool,
    pub priority: i32,
    #[serde(default)]
    pub tags: Vec<i64>,
    #[serde(default)]
    pub fields: serde_json::Value,
}
```

- [ ] **Step 2: Replace `radarr/types/indexer.rs`**

```rust
//! Indexer types (re-exported from servarr).

pub use crate::servarr::types::indexer::*;
```

- [ ] **Step 3: Replace `sonarr/types/indexer.rs`**

```rust
//! Indexer types (re-exported from servarr).

pub use crate::servarr::types::indexer::*;
```

- [ ] **Step 4: Replace `prowlarr/types/indexers.rs`**

```rust
//! Indexer types (re-exported from servarr).

pub use crate::servarr::types::indexer::*;
```

- [ ] **Step 5: Verify**

Run: `cargo check -p lab-apis --features "radarr sonarr prowlarr servarr"`
Expected: `indexer` module clean.

- [ ] **Step 6: Commit**

```bash
git add crates/lab-apis/src/servarr/types/indexer.rs crates/lab-apis/src/radarr/types/indexer.rs crates/lab-apis/src/sonarr/types/indexer.rs crates/lab-apis/src/prowlarr/types/indexers.rs
git commit -m "refactor(servarr): extract indexer types"
```

---

### Task 10: Extract `language` types

**Files:**
- Create: `crates/lab-apis/src/servarr/types/language.rs`
- Modify: `crates/lab-apis/src/radarr/types/language.rs`
- Modify: `crates/lab-apis/src/sonarr/types/language.rs`
- Modify: `crates/lab-apis/src/prowlarr/types/language.rs`

- [ ] **Step 1: Write `servarr/types/language.rs`**

```rust
//! Language types.

use serde::{Deserialize, Serialize};

/// Newtype wrapper around a language id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LanguageId(pub i32);

/// A language known to an *arr service (for audio / subtitle tracking).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Language {
    pub id: LanguageId,
    pub name: String,
    #[serde(default)]
    pub name_lower: Option<String>,
}
```

- [ ] **Step 2: Replace `radarr/types/language.rs`**

```rust
//! Language types (re-exported from servarr).

pub use crate::servarr::types::language::*;
```

- [ ] **Step 3: Replace `sonarr/types/language.rs`**

```rust
//! Language types (re-exported from servarr).

pub use crate::servarr::types::language::*;
```

- [ ] **Step 4: Replace `prowlarr/types/language.rs`**

```rust
//! Language types (re-exported from servarr).

pub use crate::servarr::types::language::*;
```

- [ ] **Step 5: Verify**

Run: `cargo check -p lab-apis --features "radarr sonarr prowlarr servarr"`
Expected: `language` module clean.

- [ ] **Step 6: Commit**

```bash
git add crates/lab-apis/src/servarr/types/language.rs crates/lab-apis/src/radarr/types/language.rs crates/lab-apis/src/sonarr/types/language.rs crates/lab-apis/src/prowlarr/types/language.rs
git commit -m "refactor(servarr): extract language types"
```

---

### Task 11: Extract `auth` types

**Files:**
- Create: `crates/lab-apis/src/servarr/types/auth.rs`
- Modify: `crates/lab-apis/src/radarr/types/auth.rs`
- Modify: `crates/lab-apis/src/sonarr/types/auth.rs`

- [ ] **Step 1: Write `servarr/types/auth.rs`**

```rust
//! Session auth types.

use serde::Serialize;

/// Body for `POST /login` when an *arr service is configured with forms auth.
#[derive(Debug, Clone, Serialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remember_me: Option<bool>,
}
```

- [ ] **Step 2: Replace `radarr/types/auth.rs`**

```rust
//! Auth types (re-exported from servarr).

pub use crate::servarr::types::auth::*;
```

- [ ] **Step 3: Replace `sonarr/types/auth.rs`**

```rust
//! Auth types (re-exported from servarr).

pub use crate::servarr::types::auth::*;
```

- [ ] **Step 4: Verify**

Run: `cargo check -p lab-apis --features "radarr sonarr servarr"`
Expected: `auth` module clean.

- [ ] **Step 5: Commit**

```bash
git add crates/lab-apis/src/servarr/types/auth.rs crates/lab-apis/src/radarr/types/auth.rs crates/lab-apis/src/sonarr/types/auth.rs
git commit -m "refactor(servarr): extract auth types"
```

---

### Task 12: Extract `release` types

**Files:**
- Create: `crates/lab-apis/src/servarr/types/release.rs`
- Modify: `crates/lab-apis/src/radarr/types/release.rs`
- Modify: `crates/lab-apis/src/sonarr/types/release.rs`

- [ ] **Step 1: Write `servarr/types/release.rs`**

```rust
//! Release (indexer search result) types.

use serde::{Deserialize, Serialize};

/// One release returned from an indexer search.
///
/// Mirrors `ReleaseResource` from the Radarr v3 / Sonarr v3 OpenAPI specs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub guid: String,
    pub title: String,
    pub indexer: String,
    pub indexer_id: i64,
    pub size: i64,
    pub age: i32,
    #[serde(default)]
    pub age_hours: f64,
    pub protocol: String,
    pub download_url: Option<String>,
    pub info_url: Option<String>,
    #[serde(default)]
    pub seeders: Option<i32>,
    #[serde(default)]
    pub leechers: Option<i32>,
    #[serde(default)]
    pub approved: bool,
    #[serde(default)]
    pub rejected: bool,
    #[serde(default)]
    pub rejections: Vec<String>,
}
```

- [ ] **Step 2: Replace `radarr/types/release.rs`**

```rust
//! Release types (re-exported from servarr).

pub use crate::servarr::types::release::*;
```

- [ ] **Step 3: Replace `sonarr/types/release.rs`**

```rust
//! Release types (re-exported from servarr).

pub use crate::servarr::types::release::*;
```

- [ ] **Step 4: Verify**

Run: `cargo check -p lab-apis --features "radarr sonarr servarr"`
Expected: `release` module clean.

- [ ] **Step 5: Commit**

```bash
git add crates/lab-apis/src/servarr/types/release.rs crates/lab-apis/src/radarr/types/release.rs crates/lab-apis/src/sonarr/types/release.rs
git commit -m "refactor(servarr): extract release types"
```

---

### Task 13: Extract `root_folder` types

**Files:**
- Create: `crates/lab-apis/src/servarr/types/root_folder.rs`
- Modify: `crates/lab-apis/src/radarr/types/root_folder.rs`
- Modify: `crates/lab-apis/src/sonarr/types/root_folder.rs`

- [ ] **Step 1: Write `servarr/types/root_folder.rs`**

```rust
//! Root folder types.

use serde::{Deserialize, Serialize};

/// Newtype wrapper around a root-folder id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RootFolderId(pub i64);

/// A root folder an *arr service stores media under.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootFolder {
    pub id: RootFolderId,
    pub path: String,
    pub accessible: bool,
    pub free_space: i64,
    pub total_space: i64,
    #[serde(default)]
    pub unmapped_folders: serde_json::Value,
}
```

- [ ] **Step 2: Replace `radarr/types/root_folder.rs`**

```rust
//! Root folder types (re-exported from servarr).

pub use crate::servarr::types::root_folder::*;
```

- [ ] **Step 3: Replace `sonarr/types/root_folder.rs`**

```rust
//! Root folder types (re-exported from servarr).

pub use crate::servarr::types::root_folder::*;
```

- [ ] **Step 4: Verify**

Run: `cargo check -p lab-apis --features "radarr sonarr servarr"`
Expected: `root_folder` module clean.

- [ ] **Step 5: Commit**

```bash
git add crates/lab-apis/src/servarr/types/root_folder.rs crates/lab-apis/src/radarr/types/root_folder.rs crates/lab-apis/src/sonarr/types/root_folder.rs
git commit -m "refactor(servarr): extract root_folder types"
```

---

### Task 14: Extract `tag` types (partial — `Tag` + `TagId` only, keep `TagDetail` in radarr)

**Files:**
- Create: `crates/lab-apis/src/servarr/types/tag.rs`
- Modify: `crates/lab-apis/src/radarr/types/tag.rs`
- Modify: `crates/lab-apis/src/sonarr/types/tag.rs`
- Modify: `crates/lab-apis/src/prowlarr/types/tags.rs` (plural)

- [ ] **Step 1: Write `servarr/types/tag.rs`**

```rust
//! Tag types.

use serde::{Deserialize, Serialize};

/// Newtype wrapper around a tag id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TagId(pub i64);

/// A free-form label attached to resources (movies, series, indexers, …).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tag {
    pub id: TagId,
    pub label: String,
}
```

- [ ] **Step 2: Rewrite `radarr/types/tag.rs` — re-export `Tag`/`TagId`, keep radarr-specific `TagDetail`**

```rust
//! Tag types. `Tag` and `TagId` live in servarr; `TagDetail` stays here
//! because its resource-id arrays are radarr-specific.

use serde::{Deserialize, Serialize};

pub use crate::servarr::types::tag::{Tag, TagId};

/// A tag plus the ids of every radarr resource currently carrying it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagDetail {
    pub id: TagId,
    pub label: String,
    #[serde(default)]
    pub movie_ids: Vec<i64>,
    #[serde(default)]
    pub indexer_ids: Vec<i64>,
    #[serde(default)]
    pub download_client_ids: Vec<i64>,
    #[serde(default)]
    pub notification_ids: Vec<i64>,
}
```

- [ ] **Step 3: Replace `sonarr/types/tag.rs` with re-export**

```rust
//! Tag types (re-exported from servarr).

pub use crate::servarr::types::tag::*;
```

- [ ] **Step 4: Replace `prowlarr/types/tags.rs`**

```rust
//! Tag types (re-exported from servarr).

pub use crate::servarr::types::tag::*;
```

- [ ] **Step 5: Verify**

Run: `cargo check -p lab-apis --features "radarr sonarr prowlarr servarr"`
Expected: `tag` module clean; `TagDetail` still resolves inside radarr.

- [ ] **Step 6: Commit**

```bash
git add crates/lab-apis/src/servarr/types/tag.rs crates/lab-apis/src/radarr/types/tag.rs crates/lab-apis/src/sonarr/types/tag.rs crates/lab-apis/src/prowlarr/types/tags.rs
git commit -m "refactor(servarr): extract Tag/TagId, keep radarr TagDetail"
```

---

### Task 15: Extract `filesystem` types (partial — entry/listing only, keep radarr `ManualImportItem`)

**Files:**
- Create: `crates/lab-apis/src/servarr/types/filesystem.rs`
- Modify: `crates/lab-apis/src/radarr/types/filesystem.rs`
- Modify: `crates/lab-apis/src/sonarr/types/filesystem.rs`
- Modify: `crates/lab-apis/src/prowlarr/types/filesystem.rs`

- [ ] **Step 1: Write `servarr/types/filesystem.rs`**

```rust
//! Filesystem browsing types.

use serde::{Deserialize, Serialize};

/// Whether a filesystem entry is a folder or a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilesystemEntryType {
    File,
    Folder,
    Drive,
}

/// One entry in a filesystem browse response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesystemEntry {
    pub name: String,
    pub path: String,
    #[serde(rename = "type")]
    pub kind: FilesystemEntryType,
    #[serde(default)]
    pub last_modified: Option<String>,
    #[serde(default)]
    pub size: Option<i64>,
}

/// Response from `/api/v3/filesystem?path=...`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesystemListing {
    pub parent: Option<String>,
    pub directories: Vec<FilesystemEntry>,
    pub files: Vec<FilesystemEntry>,
}
```

- [ ] **Step 2: Rewrite `radarr/types/filesystem.rs` — re-export shared, keep `ManualImportItem`**

```rust
//! Filesystem types. Browse shapes live in servarr; `ManualImportItem`
//! stays here because it references `MovieId`.

use serde::{Deserialize, Serialize};

pub use crate::servarr::types::filesystem::{
    FilesystemEntry, FilesystemEntryType, FilesystemListing,
};

use super::movie::MovieId;

/// One item available for manual import into radarr.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualImportItem {
    pub id: i64,
    pub path: String,
    #[serde(default)]
    pub relative_path: Option<String>,
    pub name: String,
    pub size: i64,
    #[serde(default)]
    pub movie_id: Option<MovieId>,
    #[serde(default)]
    pub quality: serde_json::Value,
    #[serde(default)]
    pub rejections: serde_json::Value,
}
```

- [ ] **Step 3: Replace `sonarr/types/filesystem.rs`**

```rust
//! Filesystem types (re-exported from servarr).

pub use crate::servarr::types::filesystem::*;
```

- [ ] **Step 4: Replace `prowlarr/types/filesystem.rs`**

```rust
//! Filesystem types (re-exported from servarr).

pub use crate::servarr::types::filesystem::*;
```

- [ ] **Step 5: Verify**

Run: `cargo check -p lab-apis --features "radarr sonarr prowlarr servarr"`
Expected: all three *arr features compile. No other missing-module errors should remain from earlier tasks.

- [ ] **Step 6: Commit**

```bash
git add crates/lab-apis/src/servarr/types/filesystem.rs crates/lab-apis/src/radarr/types/filesystem.rs crates/lab-apis/src/sonarr/types/filesystem.rs crates/lab-apis/src/prowlarr/types/filesystem.rs
git commit -m "refactor(servarr): extract filesystem browse types"
```

---

### Task 16: Wire servarr into the *arr feature list

**Files:**
- Modify: `crates/lab-apis/Cargo.toml`

Purpose: each *arr feature must activate `servarr` so downstream callers only need `--features radarr` (not `--features "radarr servarr"`).

- [ ] **Step 1: Inspect current feature declarations**

Run: `grep -n "^radarr\|^sonarr\|^prowlarr\|^servarr" crates/lab-apis/Cargo.toml`
Expected: shows current lines for `radarr = [...]`, `sonarr = [...]`, `prowlarr = [...]`, `servarr = [...]`.

- [ ] **Step 2: Edit `Cargo.toml` to make *arr features depend on servarr**

Change these three lines (exact existing form may vary — update in place):

```toml
radarr = ["servarr"]
sonarr = ["servarr"]
prowlarr = ["servarr"]
```

Leave `servarr = []` as-is.

- [ ] **Step 3: Verify the feature-activation chain works**

Run: `cargo check -p lab-apis --features radarr`
Expected: clean build — enabling `radarr` alone should now pull in `servarr` automatically.

- [ ] **Step 4: Verify the full workspace still builds with `all`**

Run: `cargo check -p lab-apis --features all`
Expected: clean build.

- [ ] **Step 5: Commit**

```bash
git add crates/lab-apis/Cargo.toml
git commit -m "chore(cargo): wire servarr into *arr feature chain"
```

---

### Task 17: Final full-workspace verification

**Files:** none — verification only.

- [ ] **Step 1: Clippy clean**

Run: `cargo clippy -p lab-apis --features all -- -D warnings`
Expected: no warnings.

- [ ] **Step 2: Format clean**

Run: `cargo fmt --check`
Expected: no diff.

- [ ] **Step 3: Workspace build**

Run: `just check`
Expected: clean.

- [ ] **Step 4: Commit (only if fmt/clippy surfaced fixups in Steps 1–2)**

```bash
git add -u crates/lab-apis
git commit -m "style(servarr): clippy and fmt fixups"
```

---

## Self-Review

**Spec coverage:** All 13 originally-listed shared types are covered — 11 clean extractions (Tasks 3–13) + 2 partial extractions (Tasks 14, 15). Servarr module scaffolding is Tasks 1–2. Feature wiring is Task 16. Final verification is Task 17. The out-of-scope set (`history`, `calendar`, `import_list`) is explicitly called out and never touched.

**Placeholder scan:** No TBD / TODO / "implement later" / vague error-handling language. Every code step contains the full file body it produces. Every verify step has an exact command and expected outcome.

**Type consistency:** `QualityProfileId` used consistently across tasks; `CommandStatus` enum variants match between the original radarr file and the servarr version; `Tag` / `TagId` split is consistent in Task 14 (servarr owns the simple shape, radarr keeps `TagDetail` with explicit re-import of `TagId`). `ManualImportItem` in Task 15 correctly imports `MovieId` via `use super::movie::MovieId`.

**Known edge cases flagged:** prowlarr plural filenames (handled without renames), `Notification`'s movie-specific flags (kept in shared type with a doc note), `TagDetail`'s radarr-specific id arrays (kept in radarr), `ManualImportItem`'s `MovieId` reference (kept in radarr).
