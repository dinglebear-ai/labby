---
title: "Snippet UI"
created: "2026-10-01"
updated: "2026-10-01"
---

# Snippet authoring and execution history

The Snippets page shows effective snippets and invalid-file diagnostics. A user override replaces the built-in row; an invalid override remains a diagnostic rather than an executable fallback.

The guided builder loads each selected tool's current input schema through the existing Tools describe API. Simple object schemas provide typed fields, required markers, enum choices, and snippet-input selectors. Complex schemas and missing schema projections retain the JSON editor. Supported top-level constraints are checked before draft generation; the backend validates execution. Tool parameter objects are separate, and independent calls use deferred `codemode.batch` jobs.

Each selected snippet has owner-scoped persisted run history. History is paginated, refreshed after execution and live testing, and reset when the snippet or browser authority changes. Receipt detail shows status, runtime, digests, indexed call outcomes, timings, and artifact references. Receipt retention and artifact-byte retention are separate contracts; missing history, disabled persistence, and failed authorized reads remain visible.

The interface uses Aurora controls and responsive wrapping. History lists use buttons with explicit selection state; typed fields have labels and required markers; load/error/empty states are text that assistive technology can read. Advanced JSON remains available for schemas the form cannot fully express.

Browser mock previews serve an isolated, task-owned copy of the static export. Later production builds cannot replace the served mock assets. The intentional build-skew test publishes a fresh snapshot into that preview directory. Compilation phases remain sequential because Next.js builds share `.next` and `out`.

## Named workflows and execution review

The builder searches the authorized catalog and adds distinct named steps, including repeated calls to the same tool. Schemas load automatically and duplicate tool descriptions are coalesced within the current authority. Refreshing a schema preserves edited mappings. Required gaps appear inline; declaring an input keeps it required without saving an invented default. Unique compatible name/type matches and non-sensitive schema defaults are explicit suggestions the author can accept or edit. Complex schemas and manual output paths remain available through Advanced JSON.

Explicit named dependencies and `$steps.stepId.path` references determine execution waves. Moving a step keeps its identity; removing or renaming a referenced step surfaces validation errors. Each wave uses deferred batches, retains individual statuses, and skips dependent steps after prerequisite failure. The static plan displays redacted known parameters and unresolved output selectors. Recognized saved builder source can return to the guided editor with optimistic digest checking; expert source stays editable through the source editor.

Execute, Save and run, and history replay open a preview before any execution. A fresh backend fingerprint binds source, inputs, current authority, runtime, and tool schemas. Run now sends that guard; replay requires acknowledgment of every changed or unverifiable drift field. Current defaults are the only replay starting values, and defaults containing nested credentials are omitted. Caller inputs remain ephemeral and are never added to history or local storage. Dynamic JavaScript previews describe declared permissions and input keys; actual calls and parameters remain unresolved. Known builder parameters are displayed only after bounded schema discovery; missing schemas retain an explicit unknown state. Sensitive names, password formats, write-only fields, and mapped input aliases are masked recursively.
