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
