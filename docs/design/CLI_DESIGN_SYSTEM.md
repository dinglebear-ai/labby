---
title: "CLI Design System Contract"
created: "2026-07-30"
updated: "2026-07-30"
---

# CLI Design System Contract

**Status:** Active
**Scope:** `labby` CLI human-readable output only
**Mode:** Dark-first, truecolor-first with ANSI-256 fallback

## Purpose

This document is the source of truth for the `labby` CLI visual language.

It defines how human-readable terminal output should communicate hierarchy, status, and interaction without drifting into per-command styling. The CLI should feel like the terminal sibling of Labby's Aurora system: calm, dense, operator-friendly, and restrained.

This contract applies to human-readable output only. JSON output remains machine-readable and unstyled.

Related documents:

- [CLI](../surfaces/CLI.md)
- [SERIALIZATION](./SERIALIZATION.md)
- Implementation: `crates/labby/src/output/` and `crates/labby/src/cli/style.rs`
- [Labby Design System Contract](./design-system-contract.md)

## Product Direction

The CLI uses the Aurora terminal language.

Aurora terminal output should feel:

- dark-first
- precise, not chatty
- premium, but not ornamental
- dense enough for operators
- calm under normal operation

Aurora terminal output should avoid:

- neon success/error colors
- decorative ASCII chrome
- full-screen color flooding
- inconsistent per-command palettes
- styling that breaks pipes, redirection, or machine tooling

## Output Modes

The CLI has two output classes:

- human-readable output
- JSON

Rules:

- JSON is never styled
- human-readable output uses semantic styling only when the render policy allows it
- all human-readable styling must degrade cleanly to plain monochrome text

## Color Policy And Environment Awareness

The CLI must remain pipeable by default.

Supported color policy values:

- `auto` — default
- `plain`
- `color`

Rules:

- `auto` styles output only when `stdout` is a TTY and `NO_COLOR` is unset
- `plain` disables styling even on TTYs
- `color` forces styling for users who explicitly want it
- non-TTY output in `auto` mode must render plain text with no ANSI escapes
- redirected, piped, or captured output must stay readable without color
- all commands must use the same shared policy; command-local color heuristics are banned

## Semantic Token Contract

Commands must use semantic tokens rather than raw ANSI values or ad hoc `red/green/blue` choices.

### Foundation

- `cli.bg.page`
- `cli.bg.panel`
- `cli.bg.panel_strong`
- `cli.bg.control`
- `cli.border.default`
- `cli.border.strong`
- `cli.text.primary`
- `cli.text.muted`

### Accent

- `cli.accent.primary`
- `cli.accent.strong`
- `cli.accent.deep`

### State

- `cli.state.success`
- `cli.state.warn`
- `cli.state.error`
- `cli.state.info`

### Interaction

- `cli.focus`
- `cli.selection`
- `cli.active`
- `cli.disabled`

## Palette Contract

### Truecolor Values

- `cli.bg.page: #07131c`
- `cli.bg.panel: #102330`
- `cli.bg.panel_strong: #13293a`
- `cli.bg.control: #0c1a24`
- `cli.border.default: #1d3d4e`
- `cli.border.strong: #24536c`
- `cli.text.primary: #e6f4fb`
- `cli.text.muted: #a7bcc9`
- `cli.accent.primary: #29b6f6`
- `cli.accent.strong: #67cbfa`
- `cli.accent.deep: #1c7fac`
- `cli.state.success: #7dd3c7`
- `cli.state.warn: #c6a36b`
- `cli.state.error: #c78490`
- `cli.state.info: #67cbfa`

### ANSI-256 Fallback

- `cli.bg.page -> 233`
- `cli.bg.panel -> 235`
- `cli.bg.panel_strong -> 236`
- `cli.bg.control -> 234`
- `cli.border.default -> 239`
- `cli.border.strong -> 31`
- `cli.text.primary -> 255`
- `cli.text.muted -> 250`
- `cli.accent.primary -> 39`
- `cli.accent.strong -> 81`
- `cli.accent.deep -> 31`
- `cli.state.success -> 115`
- `cli.state.warn -> 180`
- `cli.state.error -> 174`
- `cli.state.info -> 81`

Rules:

- truecolor is the primary target
- ANSI-256 fallback must preserve hierarchy, not exact hue
- commands must never hardcode raw palette values in render paths

## Text Role Contract

The web typography split becomes terminal text roles.

### Display

Use for:

- command-level titles
- top-level summary headings

Rules:

- bold
- `cli.text.primary`
- no decorative punctuation framing

### Section

Use for:

- grouped subsections
- table titles
- named output blocks

Rules:

- bold
- primary or accent emphasis

### Metric

Use for:

- totals
- health summaries
- counts
- latency rollups

Rules:

- tabular alignment when possible
- accent only when emphasis is useful
- do not paint entire metric dashboards bright by default

### Body

Use for:

- standard content
- explanations
- normal values

Rules:

- `cli.text.primary`

### Control

Use for:

- flags
- option names
- command hints
- interactive prompts

Rules:

- labels muted, values primary

### Dense

Use for:

- tables
- logs
- inspectors
- key/value rows

Rules:

- compact spacing
- monospace alignment
- muted separators

### Eyebrow

Use for:

- small categorical labels above a section
- context labels such as `SERVICE HEALTH` or `EXTRACT REPORT`

Rules:

- uppercase
- muted
- compact

## Hierarchy And Surface Translation

The CLI cannot use web shadows or radii. It must preserve the same hierarchy through spacing, borders, and density.

### Tier 0

Use for:

- plain page canvas
- top-level output flow

Rules:

- prefer unboxed content
- avoid heavy separator noise

### Tier 1

Use for:

- support panels
- option summaries
- auxiliary sections

Rules:

- subtle border or divider
- muted tone
- compact padding

### Tier 2

Use for:

- primary inspectors
- important summaries
- selected entity detail

Rules:

- stronger border
- stronger heading emphasis
- use sparingly so primary surfaces remain obvious

## Spacing Contract

The CLI uses compact operator spacing.

- `1` space for inline separators
- `2` spaces for key/value alignment gutters
- `1` blank line between major groups only
- no decorative vertical whitespace

Rules:

- favor scanability over air
- do not insert blank lines between every row
- group related information tightly

## Symbol Contract

Use a restrained symbol set.

Preferred:

- `•` bullet
- `▸` nested section marker
- `›` focus marker in interactive views
- `─` divider
- `✓` success
- `!` warning
- `x` error
- `…` truncation

Rules:

- all symbols must have an ASCII-safe fallback where output may land in constrained environments
- symbols reinforce hierarchy but must not become decoration

## Status Guidance

Status accents must remain muted and integrated into the Aurora family.

### Success

- means healthy, connected, authorized, or completed
- calm teal
- not bright green by default

### Warning

- means degraded, partial, pending, or attention-needed
- informational before alarming

### Error

- means failed, unreachable, rejected, or destructive risk
- serious without becoming the loudest surface on every screen

### Accent

- reserved for primary action, focus, or key summary emphasis
- not a replacement for status colors

Rules:

- color alone must never carry state
- every status color must pair with a label, icon, or explicit word

## Component Contract

### Headings

Rules:

- top-level command output gets one clear title
- section headings must be visually calmer than route-level titles

### Key/Value Rows

Rules:

- labels muted
- values primary
- align rows for scanning

### Tables

Rules:

- compact rows
- muted separators
- truncation by default where scanability matters
- wrapping allowed only in clearly expanded views

### Status Badges

Rules:

- compact and quiet
- use state token plus explicit text
- avoid saturated filled blocks in normal output

### Panels

Rules:

- use only when they improve grouping or comparison
- primary panels use Tier 2
- support panels use Tier 1
- do not box every section by default

### Hints And Next Actions

Rules:

- hints use muted styling
- actionable commands or flags should be primary or accent
- hints must remain skimmable and short

## Accessibility And Pipeability

Minimum rules:

- output must remain understandable in monochrome
- status must not rely on color alone
- muted text must stay readable on dark terminals
- plain mode and non-TTY output must produce clean text with no escape sequences
- JSON output is the only machine-readable structured output mode; human output should still remain easy to grep and copy

## Engineering Rules

When implementing or refactoring CLI output:

- use shared semantic tokens before adding any new style helper
- do not hardcode raw ANSI escapes in command handlers
- do not make command-local color decisions
- keep output formatting centralized in the output layer
- add new tokens or text roles in the shared CLI theme API, not inside individual commands
- environment-aware color policy must be resolved once per render path and passed through
- use plain text fallbacks for every styled construct

## Approval Rule

A CLI output surface is not aligned unless it satisfies all of the following:

- uses the shared semantic token system
- honors the shared color policy
- remains readable when piped or redirected
- preserves dense operator-first hierarchy
- keeps status accents muted and explicit
- avoids incompatible per-command styling languages

When in doubt, extend the shared CLI design contract and API rather than letting a single command invent a new output language.
