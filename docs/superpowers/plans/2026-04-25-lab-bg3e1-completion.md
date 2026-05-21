# lab-bg3e.1 Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish bead `lab-bg3e.1` by making `PluginMeta`/`EnvVar` carry complete const-friendly UI schema metadata and proving the covered service set has no missing field schema metadata.

**Architecture:** Keep schema source-of-truth in the pure SDK crate as `core/plugin_ui.rs` with only const-friendly `Copy` types and static leaves. Keep enforcement in the binary audit layer, which scans service entry files for missing `EnvVar.ui` metadata and stale `PluginMeta.supports_multi_instance` wiring.

**Tech Stack:** Rust 2024, `lab-apis` core metadata, `lab` audit/scaffold code, existing `lab audit onboarding`, shell verification scripts.

---

## File Structure

- Modify: `crates/lab-apis/src/core/plugin_ui.rs` - replace the partial UI hint shape with the bead-locked `UiSchema` shape, field kinds, validation defaults, and file path validation helper/tests.
- Modify: `crates/lab-apis/src/core.rs` - re-export any new `plugin_ui` constants/types used by services, scaffolded code, or tests.
- Modify: `crates/lab/src/audit/checks.rs` - register the new UI schema audit check module.
- Create: `crates/lab/src/audit/checks/ui_schema.rs` - static onboarding check for `supports_multi_instance`, `EnvVar.ui: Some(...)`, and local `help_url` scheme rules.
- Modify: `crates/lab/src/audit/onboarding.rs` - include the UI schema check in each service report.
- Modify: `crates/lab/src/scaffold/templates/lab_apis_service.tpl` - emit `EnvVar` entries with explicit `UiSchema` constants plus `supports_multi_instance`.
- Modify: `crates/lab-apis/src/extract/CLAUDE.md` - retire stale `Category::Bootstrap` only-member instruction.
- Modify: `crates/lab-apis/CLAUDE.md` - document current Bootstrap peers and feature-count drift accurately enough for this bead.
- Create: `docs/sessions/2026-04-25-lab-bg3e1-completion.md` - factual session report with requested command context and verification evidence.

## Service Scope

The bead-covered 23 metadata services are:

`radarr`, `sonarr`, `prowlarr`, `overseerr`, `tautulli`, `arcane`, `plex`, `sabnzbd`, `qbittorrent`, `unifi`, `qdrant`, `tei`, `tailscale`, `apprise`, `gotify`, `bytestash`, `linkding`, `memos`, `openai`, `paperless`, `unraid`, `extract`, `device_runtime`.

Current repo extras with `PluginMeta` are also checked for regressions where practical:

`deploy`, `mcpregistry`, `acp_registry`, `doctor`, `marketplace`, `acp`.

### Task 1: Normalize SDK UI schema shape

**Files:**
- Modify: `crates/lab-apis/src/core/plugin_ui.rs`
- Modify: `crates/lab-apis/src/core.rs`

- [ ] **Step 1: Replace `UiSchema` fields with the locked bead contract.**

Implement:

```rust
#[derive(Debug, Clone, Copy, Default)]
pub struct UiSchema {
    pub kind: FieldKind,
    pub validation: FieldValidation,
    pub advanced: bool,
    pub help_url: Option<&'static str>,
    pub depends_on: Option<&'static str>,
    pub wizard_kind: Option<WizardKind>,
    pub dynamic_source: Option<&'static str>,
}
```

- [ ] **Step 2: Add const defaults and const-friendly field kinds.**

Implement `FieldKind::{Text, Secret, Url, Bool, Number, FilePath, Enum { values }}` with a `Default` of `Text`, `FieldValidation` with static optional constraints, and constants like `UI_SCHEMA_DEFAULT` and `FIELD_VALIDATION_DEFAULT` for const struct update syntax.

- [ ] **Step 3: Rebuild existing helper constants.**

Update `URL_FIELD`, `SECRET_FIELD`, `SECRET_OPTIONAL_FIELD`, `URL_OPTIONAL_FIELD`, `TEXT_FIELD`, `TEXT_OPTIONAL_FIELD`, and `BOOL_FIELD` to use the new fields and `advanced` defaults explicitly.

- [ ] **Step 4: Add file path validation helper and tests.**

Add a helper that rejects `Component::ParentDir` and verifies the final joined path starts with the caller-provided safe root. Add unit tests for parent-dir rejection, safe relative path acceptance, root escape rejection, and `UiSchema::default().advanced == false`.

### Task 2: Add audit enforcement for UI schema completeness

**Files:**
- Create: `crates/lab/src/audit/checks/ui_schema.rs`
- Modify: `crates/lab/src/audit/checks.rs`
- Modify: `crates/lab/src/audit/onboarding.rs`

- [ ] **Step 1: Implement a static check.**

For `crates/lab-apis/src/{service}.rs`, report pass/fail for:

- `metadata.supports_multi_instance`: the `PluginMeta` block includes `supports_multi_instance:`.
- `metadata.ui_schema`: every literal `EnvVar { ... }` block includes `ui: Some(`.
- `metadata.help_url`: any literal `help_url: Some("...")` uses `https://`, or `http://localhost`, `http://127.0.0.1`, or `http://[::1]`.

- [ ] **Step 2: Register the check in onboarding audit.**

Extend `audit_service()` so `lab audit onboarding <service>` reports the new metadata checks alongside existing file, registration, dispatch, tests, and docs checks.

- [ ] **Step 3: Add focused unit tests.**

Test that missing `ui`, `ui: None`, missing `supports_multi_instance`, valid help URLs, and invalid help URLs produce expected results.

### Task 3: Update scaffold and docs

**Files:**
- Modify: `crates/lab/src/scaffold/templates/lab_apis_service.tpl`
- Modify: `crates/lab-apis/src/extract/CLAUDE.md`
- Modify: `crates/lab-apis/CLAUDE.md`

- [ ] **Step 1: Update service scaffold metadata.**

Import `EnvVar`, `URL_FIELD`, and `SECRET_OPTIONAL_FIELD`; emit a URL required env var and optional API key env var with explicit `ui: Some(...)`; include `supports_multi_instance: false`.

- [ ] **Step 2: Retire stale Bootstrap-only wording.**

Update extract docs so Bootstrap includes `extract`, `deploy`, `doctor`, and `device_runtime` style bootstrap/operator services rather than claiming extract is the only member.

- [ ] **Step 3: Update lab-apis feature/Bootstrap note.**

Clarify that feature counts can drift with new registry/bootstrap integrations and that Bootstrap has multiple operator peers.

### Task 4: Verify bead completion

**Files:**
- Create: `docs/sessions/2026-04-25-lab-bg3e1-completion.md`

- [ ] **Step 1: Run metadata completeness script for the 23-service bead scope.**

Run a shell/Python check that fails if any bead-scoped service file is missing, lacks `supports_multi_instance:`, has an `EnvVar` without `ui: Some(`, or has an invalid literal `help_url`.

- [ ] **Step 2: Run metadata completeness script for extra current `PluginMeta` modules.**

Run the same check for `deploy`, `mcpregistry`, `acp_registry`, `doctor`, `marketplace`, and `acp`; document whether any extra current modules intentionally have no env vars.

- [ ] **Step 3: Run targeted Rust tests.**

Run `cargo test -p lab-apis plugin_ui --all-features` and `cargo test -p lab audit::checks::ui_schema --all-features` or the closest exact test filters supported by Cargo.

- [ ] **Step 4: Run real audit command for representative services.**

Run `cargo run -p lab --all-features -- audit onboarding radarr mcpregistry --json` and confirm the new metadata checks pass. If unrelated onboarding checks fail, record that separately and rely on the targeted metadata script for bead closure.

- [ ] **Step 5: Run all-features build.**

Run `cargo build --all-features` to prove the const metadata changes compile across every feature-gated service.

- [ ] **Step 6: Write session report.**

Create the requested report with command context, files modified, verification evidence, risks, and open questions. Include the active plan path.
