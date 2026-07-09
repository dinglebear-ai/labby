/**
 * Shared Aurora design-system class-string constants.
 *
 * Source of truth for token naming:
 *   - CSS vars:    app/globals.css
 *   - Semantics:   docs/design/design-system-contract.md
 *
 * One-way dependency rule:
 *   components/aurora/** must NOT import from components/ui/**
 *   This module depends on nothing in the primitive tree.
 *   Primitives may depend on this module.
 */

// ---------------------------------------------------------------------------
// Typography
// ---------------------------------------------------------------------------

/** Display 1 — page titles. Manrope, 34px, extrabold, tight tracking. */
export const AURORA_DISPLAY_1 =
  'font-display text-[34px] leading-[1.04] font-extrabold'

/** Display 2 — section headers. Manrope, 19px, bold, moderate tracking. */
export const AURORA_DISPLAY_2 =
  'font-display text-[19px] leading-[1.12] font-bold'

/**
 * Display title — section-level heading (legacy alias for AURORA_DISPLAY_2).
 * Kept for backwards compat with gateway-theme.ts and settings page.
 */
export const AURORA_DISPLAY_TITLE = 'font-display'

/** Metric Display — large numbers, dashboard stats. Manrope, tabular. */
export const AURORA_DISPLAY_NUMBER =
  'font-display text-[28px] leading-none font-extrabold tabular-nums'

/** Card title — dense catalog/list card titles. Manrope, compact and emphatic. */
export const AURORA_CARD_TITLE =
  'font-display text-[15px] leading-[1.16] font-extrabold'

/** Compact title — empty states and compact panel headings. Manrope, below Display 2. */
export const AURORA_COMPACT_TITLE =
  'font-display text-[17px] leading-[1.18] font-extrabold'

/**
 * Eyebrow / muted label — uppercase metadata labels.
 * Weight 700 (font-bold) per contract Eyebrow spec.
 */
export const AURORA_MUTED_LABEL =
  'text-[11px] font-bold uppercase tracking-[0.18em] text-aurora-text-muted'

/** Badge label — compact uppercase status/category badges and chips. */
export const AURORA_BADGE_LABEL =
  'text-[10px] leading-none font-bold uppercase tracking-[0.14em]'

/** Dense metadata — subtitles, package identifiers, versions, and table support text. */
export const AURORA_DENSE_META =
  'text-[11px] leading-[1.35] font-medium'

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/** Page content frame — max-width container with standard padding. */
export const AURORA_PAGE_FRAME =
  'mx-auto flex w-full max-w-[1740px] flex-col gap-5 px-3 py-5 sm:px-6 sm:py-6 xl:px-8'

/** Page background shell — subtle radial aurora gradient. */
export const AURORA_PAGE_SHELL =
  'bg-[radial-gradient(circle_at_top_left,rgba(41,182,246,0.08),transparent_26%),radial-gradient(circle_at_top_right,rgba(103,203,250,0.06),transparent_20%)]'

// ---------------------------------------------------------------------------
// Panels
// ---------------------------------------------------------------------------

/** Tier 2 (Medium) panel — standard content surfaces. */
export const AURORA_MEDIUM_PANEL =
  'rounded-aurora-3 border border-aurora-border-strong bg-aurora-panel-medium shadow-[var(--aurora-shadow-medium),var(--aurora-highlight-medium)]'

/** Tier 3 (Strong) panel — elevated/featured content surfaces. */
export const AURORA_STRONG_PANEL =
  'rounded-aurora-3 border border-aurora-border-strong bg-aurora-panel-strong shadow-[var(--aurora-shadow-strong),var(--aurora-highlight-strong)]'

/** Control surface — input/select backgrounds (Tier 1). */
export const AURORA_CONTROL_SURFACE =
  'rounded-aurora-1 border-aurora-border-strong bg-aurora-control-surface shadow-[0_8px_16px_rgba(0,0,0,0.16),var(--aurora-highlight-medium)]'

/** Inline message/code surface — floating content within panels. */
export const AURORA_MESSAGE_SURFACE =
  'rounded-[1rem] border border-aurora-border-strong bg-[linear-gradient(180deg,rgba(18,40,56,0.96),rgba(14,31,44,0.98))] shadow-[0_8px_16px_rgba(0,0,0,0.16),var(--aurora-highlight-medium)]'

/** Stat panel — compact metric/lens surfaces with dark gradient lift. */
export const AURORA_STAT_PANEL =
  'rounded-[1.05rem] border border-aurora-border-strong bg-[linear-gradient(180deg,rgba(18,40,56,0.96),rgba(12,27,38,0.98))] px-4 py-3 shadow-[0_8px_18px_rgba(0,0,0,0.18),var(--aurora-highlight-medium)]'

/** Accent CTA surface for highlighted secondary actions in product/demo panels. */
export const AURORA_ACCENT_CTA =
  'rounded-aurora-1 border border-aurora-accent-primary/35 bg-[color-mix(in_srgb,var(--aurora-accent-primary)_16%,transparent)] text-aurora-text-primary hover:bg-[color-mix(in_srgb,var(--aurora-accent-primary)_22%,transparent)]'

// ---------------------------------------------------------------------------
// Interactive tone helpers
// ---------------------------------------------------------------------------

/**
 * Pill toggle tone — filter pills, tabs, compact toggles.
 * Uses Aurora tokens only; no hardcoded hex.
 *
 * @param active - true when the pill is selected/active
 */
export function pillTone(active: boolean): string {
  return active
    ? 'border-aurora-accent-primary/28 bg-[linear-gradient(180deg,rgba(16,35,48,0.96),rgba(11,25,35,0.98))] text-aurora-text-primary shadow-[var(--aurora-active-glow)]'
    : 'border-aurora-border-strong bg-[linear-gradient(180deg,rgba(10,23,32,0.92),rgba(9,20,28,0.98))] text-aurora-text-muted shadow-[inset_0_1px_0_rgba(255,255,255,0.02)]'
}

/**
 * Control tone — action buttons, toolbar controls.
 * Uses Aurora tokens only; no hardcoded hex.
 *
 * @param variant - 'default' for neutral, 'accent' for highlighted accent state
 */
export function controlTone(variant: 'default' | 'accent' = 'default'): string {
  return variant === 'accent'
    ? 'border-aurora-accent-primary/28 bg-[linear-gradient(180deg,rgba(16,35,48,0.96),rgba(11,25,35,0.98))] text-aurora-accent-strong shadow-[var(--aurora-active-glow)]'
    : 'border-aurora-border-strong bg-[linear-gradient(180deg,rgba(18,40,56,0.96),rgba(14,31,44,0.98))] text-aurora-text-primary shadow-[0_8px_16px_rgba(0,0,0,0.16),var(--aurora-highlight-medium)]'
}

