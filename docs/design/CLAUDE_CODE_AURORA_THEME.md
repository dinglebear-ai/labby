---
title: "Aurora Theme for Claude Code"
created: "2026-07-30"
updated: "2026-07-30"
---

# Aurora Theme for Claude Code

**Status:** Active
**File:** `~/.claude/themes/aurora.json`
**Requires:** Claude Code v2.1.118+

## Purpose

This theme ports the Aurora design system palette into Claude Code's custom theme format, so the terminal tool matches the visual language of Labby and the `labby` CLI.

Related documents:

- [CLI Design System Contract](./CLI_DESIGN_SYSTEM.md)
- [Labby Design System Contract](./design-system-contract.md)

## Activation

```bash
# In Claude Code, open the theme picker:
/theme
# Select: Aurora
```

Claude Code watches `~/.claude/themes/` and hot-reloads on file change.

## Token Map

Each Claude Code color token is mapped to its Aurora design system equivalent.

| Claude Code token | Aurora token | Hex |
|---|---|---|
| `claude` | `accent.primary` | `#29b6f6` |
| `text`, `primary`, `header`, `code` | `text.primary` | `#e6f4fb` |
| `muted`, `secondary`, `footer`, `status`, `label`, `thinking` | `text.muted` | `#a7bcc9` |
| `dim` | — | `#4a6878` |
| `accent`, `highlight`, `user`, `tool`, `info` | `accent.strong` | `#67cbfa` |
| `bash` | `accent.deep` | `#1c7fac` |
| `cursor`, `prompt`, `assistant`, `link`, `badge` | `accent.primary` | `#29b6f6` |
| `border`, `separator`, `selection` | `border.default` | `#1d3d4e` |
| `tag` | `border.strong` | `#24536c` |
| `success`, `added` | `state.success` | `#7dd3c7` |
| `warning`, `permission`, `changed`, `modified` | `state.warn` | `#c6a36b` |
| `error`, `removed` | `state.error` | `#c78490` |
| `diff` | `text.primary` | `#e6f4fb` |

## Aurora Palette Reference

```
text.primary:   #e6f4fb   — near-white with a cool blue cast
text.muted:     #a7bcc9   — WCAG AA on panel surfaces
accent.primary: #29b6f6   — signature Aurora blue
accent.strong:  #67cbfa   — lighter variant for secondary accents
accent.deep:    #1c7fac   — darker variant for low-emphasis surfaces
border.default: #1d3d4e   — subtle panel boundary
border.strong:  #24536c   — stronger structural boundary
state.success:  #7dd3c7   — calm teal; not neon green
state.warn:     #c6a36b   — warm amber; informational before alarming
state.error:    #c78490   — muted rose; serious without dominating
```

## Design Intent

The mapping follows the Aurora terminal language from CLI_DESIGN_SYSTEM.md:

- **calm under normal operation** — no neon greens, no saturated reds
- **accent reserved for identity** — `claude`, `prompt`, `assistant`, and `link` all share `accent.primary` so branded surfaces read consistently
- **status stays muted** — success, warning, and error use the Aurora state family rather than system ANSI defaults
- **hierarchy through tone** — `dim` sits below `muted` to provide a three-level text hierarchy (primary → muted → dim) without introducing off-palette values

## Updating

Edit `~/.claude/themes/aurora.json` directly. Claude Code hot-reloads on save — no restart needed.

To add or adjust a token, cross-reference the Claude Code token list and Aurora palette above. Unknown tokens are silently ignored, so typos are safe to fix in place.
