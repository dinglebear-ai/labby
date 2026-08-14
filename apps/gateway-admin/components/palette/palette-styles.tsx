/**
 * Command-palette CSS, ported from the Claude Design `Gateway Console` mock.
 *
 * Every value here was read off the mock's live DOM (`agent-browser eval`), not
 * inferred from a screenshot. Selectors are scoped under `[data-palette]` and
 * rendered inside the palette's own portal, so they land after the Tailwind
 * stylesheet in document order and win equal-specificity ties without
 * `!important`. Keeping them here — rather than in `app/globals.css` — keeps the
 * palette self-contained.
 *
 * The `max-width: 900px` block reproduces the mock's bottom-dock behaviour
 * verbatim: the palette becomes a full-width sheet pinned to the bottom edge
 * with 18px top corners, an 82vh cap, and 44px minimum row height.
 */

const PALETTE_CSS = `
div[data-palette] {
  position: fixed;
  z-index: 51;
  top: 11vh;
  left: 0;
  right: 0;
  bottom: auto;
  margin: 0 auto;
  width: min(680px, calc(100vw - 48px));
  max-width: none;
  max-height: none;
  transform: none;
  display: block;
  padding: 0;
  gap: 0;
  border-radius: 16px;
  border: 1px solid color-mix(in srgb, var(--aurora-border-default) 45%, var(--aurora-page-bg));
  background: linear-gradient(180deg, var(--aurora-panel-strong-top), var(--aurora-panel-strong));
  box-shadow: var(--aurora-shadow-strong), inset 0 1px 0 rgba(255, 255, 255, 0.05);
  overflow: hidden;
}

/* ── Header ─────────────────────────────────────────────────────────────── */
[data-palette] .pal-head {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 10px 12px;
  border-bottom: 1px solid color-mix(in srgb, var(--aurora-border-default) 70%, var(--aurora-page-bg));
}
[data-palette] .pal-back {
  width: 30px;
  height: 38px;
  flex-shrink: 0;
  border-radius: 10px;
  border: none;
  background: none;
  color: var(--aurora-text-muted);
  display: grid;
  place-items: center;
  cursor: pointer;
}
[data-palette] .pal-back:hover {
  background: var(--aurora-hover-bg);
  color: var(--aurora-text-primary);
}
[data-palette] .pal-field {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 9px;
  height: 38px;
  padding: 0 10px;
  border-radius: 11px;
  border: 1px solid color-mix(in srgb, var(--aurora-border-default) 70%, var(--aurora-page-bg));
  background: var(--aurora-control-surface);
  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.035);
  transition: border-color 150ms, box-shadow 150ms, background 150ms;
}
[data-palette] .pal-field:focus-within {
  border-color: color-mix(in srgb, var(--aurora-accent-primary) 45%, transparent);
  background: color-mix(in srgb, var(--aurora-accent-primary) 5%, var(--aurora-control-surface));
  box-shadow:
    0 0 0 3px rgba(41, 182, 246, 0.13),
    0 0 20px rgba(41, 182, 246, 0.12),
    inset 0 1px 0 rgba(255, 255, 255, 0.035);
}
[data-palette] .pal-field-icon {
  display: grid;
  place-items: center;
  flex-shrink: 0;
  color: var(--aurora-text-muted);
  transition: color 150ms;
}
[data-palette] .pal-field:focus-within .pal-field-icon {
  color: var(--aurora-accent-strong);
}
[data-palette] .pal-input {
  flex: 1;
  min-width: 0;
  height: auto;
  padding: 0;
  border: none;
  border-radius: 0;
  background: none;
  outline: none;
  font-family: inherit;
  font-size: 14px;
  color: var(--aurora-text-primary);
  caret-color: var(--aurora-accent-strong);
}
[data-palette] .pal-input::placeholder {
  color: var(--aurora-text-muted);
}
[data-palette] .pal-kbd {
  flex-shrink: 0;
  font-family: inherit;
  font-size: 9.5px;
  font-weight: 650;
  padding: 2px 5px;
  border-radius: 5px;
  border: 1px solid color-mix(in srgb, var(--aurora-border-default) 80%, transparent);
  background: var(--gw0-0_40);
  color: color-mix(in srgb, var(--aurora-text-muted) 75%, transparent);
}
[data-palette] .pal-kbd-esc {
  flex-shrink: 0;
  font-family: inherit;
  font-size: 10px;
  font-weight: 650;
  padding: 3px 6px;
  border-radius: 5px;
  border: 1px solid var(--aurora-border-default);
  background: color-mix(in srgb, var(--aurora-panel-medium) 70%, transparent);
  color: var(--aurora-text-muted);
}
[data-palette] .pal-iconbtn {
  width: 24px;
  height: 24px;
  flex-shrink: 0;
  border-radius: 7px;
  border: none;
  background: none;
  color: var(--aurora-text-muted);
  display: grid;
  place-items: center;
  cursor: pointer;
}
[data-palette] .pal-iconbtn:hover {
  color: var(--aurora-text-primary);
  background: var(--aurora-hover-bg);
}

/* ── Counts strip ───────────────────────────────────────────────────────── */
[data-palette] .pal-counts {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 14px;
  border-bottom: 1px solid color-mix(in srgb, var(--aurora-border-default) 55%, var(--aurora-page-bg));
  background: var(--gw0-0_30);
  flex-wrap: wrap;
}
[data-palette] .pal-count {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-size: 10px;
  font-weight: 650;
  letter-spacing: 0.09em;
  text-transform: uppercase;
  color: color-mix(in srgb, var(--aurora-text-muted) 78%, transparent);
  white-space: nowrap;
}
[data-palette] .pal-count b {
  color: var(--aurora-text-primary);
  font-variant-numeric: tabular-nums;
  letter-spacing: 0;
  font-weight: inherit;
}
[data-palette] .pal-scope {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 18px;
  padding: 0 7px;
  border-radius: 999px;
  border: 1px solid color-mix(in srgb, var(--aurora-accent-primary) 38%, transparent);
  background: color-mix(in srgb, var(--aurora-accent-primary) 12%, transparent);
  color: var(--aurora-accent-strong);
  font-family: inherit;
  font-size: 9.5px;
  font-weight: 700;
  letter-spacing: 0.06em;
  text-transform: uppercase;
  cursor: pointer;
}
[data-palette] .pal-hint {
  font-size: 9.5px;
  font-weight: 600;
  color: color-mix(in srgb, var(--aurora-text-muted) 60%, transparent);
  white-space: nowrap;
}

/* ── List, sections, rows ───────────────────────────────────────────────── */
[data-palette] .pal-listwrap {
  display: flex;
  align-items: stretch;
  min-width: 0;
}
[data-palette] [data-pallist] {
  flex: 1;
  min-width: 0;
  max-height: 300px;
  overflow-y: auto;
  overflow-x: hidden;
  padding: 0;
}
[data-palette] .pal-section {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 7px 14px 5px;
  border-top: 1px solid color-mix(in srgb, var(--aurora-border-default) 40%, var(--aurora-page-bg));
  background: var(--gw0-0_44);
  font-size: 9.5px;
  font-weight: 700;
  letter-spacing: 0.14em;
  text-transform: uppercase;
  color: color-mix(in srgb, var(--aurora-text-muted) 75%, transparent);
}
[data-palette] .pal-split {
  height: 4px;
  border-top: 1px solid var(--aurora-border-strong);
  background: linear-gradient(180deg, rgba(0, 0, 0, 0.28), transparent);
}
[data-palette] [data-palrow] {
  position: relative;
  display: flex;
  align-items: center;
  gap: 10px;
  box-sizing: border-box;
  width: 100%;
  padding: 9px 14px;
  border: none;
  border-top: 1px solid color-mix(in srgb, var(--aurora-border-default) 40%, var(--aurora-page-bg));
  border-radius: 0;
  background: var(--gw1-0_55);
  font-family: inherit;
  text-align: left;
  min-width: 0;
  cursor: pointer;
  transition: background 130ms ease-out, padding-left 130ms ease-out;
}
[data-palette] [data-palrow][data-palzebra='1'] {
  background: var(--gw2-0_35);
}
[data-palette] [data-palrow]::before {
  content: '';
  position: absolute;
  left: 0;
  top: 6px;
  bottom: 6px;
  width: 2.5px;
  border-radius: 999px;
  background: var(--aurora-accent-strong);
  opacity: 0;
  transform: scaleY(0.4);
  transition: opacity 130ms ease-out, transform 160ms ease-out;
}
[data-palette] [data-palrow]:hover {
  background: var(--aurora-hover-bg);
  padding-left: 19px;
}
[data-palette] [data-palrow]:hover::before {
  opacity: 0.9;
  transform: scaleY(1);
}
[data-palette] [data-palrow][data-selected='true'] {
  background: color-mix(in srgb, var(--aurora-accent-primary) 8%, transparent);
  box-shadow:
    inset 0 0 0 1px color-mix(in srgb, var(--aurora-accent-primary) 35%, transparent),
    inset 3px 0 0 color-mix(in srgb, var(--aurora-accent-primary) 55%, transparent);
}
[data-palette] [data-palrow][data-alerttone] {
  padding: 8px 14px;
  border-top-color: color-mix(in srgb, var(--aurora-border-default) 30%, var(--aurora-page-bg));
}
[data-palette] [data-palrow][data-alerttone='error'] {
  background: color-mix(in srgb, var(--aurora-error) 4%, var(--gw0-0_50));
}
[data-palette] [data-palrow][data-alerttone='warn'] {
  background: color-mix(in srgb, var(--aurora-warn) 4%, var(--gw0-0_50));
}
[data-palette] [data-palrow][data-hoverrow] {
  padding: 6px 14px;
  overflow: hidden;
}
[data-palette] .pal-chip {
  flex-shrink: 0;
  display: grid;
  place-items: center;
  width: 22px;
  height: 22px;
  border-radius: 7px;
  border: 1px solid color-mix(in srgb, var(--aurora-accent-primary) 26%, transparent);
  background: color-mix(in srgb, var(--aurora-accent-primary) 9%, transparent);
  color: var(--aurora-accent-strong);
}
[data-palette] .pal-chip[data-tone='muted'] {
  border-color: color-mix(in srgb, var(--aurora-border-strong) 70%, transparent);
  background: var(--gw0-0_45);
  color: var(--aurora-text-muted);
}
[data-palette] .pal-chip[data-tone='bare'] {
  border: none;
  background: none;
  border-radius: 0;
}
[data-palette] .pal-dot {
  width: 7px;
  height: 7px;
  flex-shrink: 0;
  border-radius: 999px;
}
[data-palette] .pal-label {
  font-size: 13px;
  font-weight: 650;
  color: var(--aurora-text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
}
[data-palette] .pal-name {
  flex-shrink: 0;
  max-width: 130px;
  overflow: hidden;
  text-overflow: ellipsis;
  font-family: var(--font-display);
  font-size: 13px;
  font-weight: 760;
  color: var(--aurora-text-primary);
  white-space: nowrap;
}
[data-palette] .pal-grow {
  flex: 1;
  min-width: 0;
}
[data-palette] .pal-sub {
  flex-shrink: 0;
  font-size: 10px;
  font-weight: 650;
  letter-spacing: 0.1em;
  text-transform: uppercase;
  color: var(--aurora-text-muted);
  white-space: nowrap;
}
[data-palette] .pal-sub[data-plain='1'] {
  font-size: 10.5px;
  letter-spacing: 0;
  text-transform: none;
}
[data-palette] .pal-endpoint {
  min-width: 0;
  font-size: 10.5px;
  color: var(--aurora-text-muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
[data-palette] .pal-rowbtn {
  width: 26px;
  height: 26px;
  flex-shrink: 0;
  border-radius: 7px;
  border: none;
  background: none;
  color: var(--aurora-text-muted);
  display: grid;
  place-items: center;
  cursor: pointer;
}
[data-palette] .pal-rowbtn[data-small='1'] {
  width: 20px;
  height: 20px;
  border-radius: 6px;
}
[data-palette] .pal-rowbtn:hover {
  background: var(--aurora-hover-bg);
  color: var(--aurora-text-primary);
}
[data-palette] .pal-transport {
  flex-shrink: 0;
  width: 40px;
  text-align: right;
  font-size: 10px;
  font-weight: 650;
  letter-spacing: 0.1em;
  text-transform: uppercase;
  color: var(--aurora-text-muted);
}
[data-palette] .pal-empty {
  padding: 22px 14px;
  text-align: center;
  font-size: 12.5px;
  color: var(--aurora-text-muted);
}

/* ── Service detail header ──────────────────────────────────────────────── */
[data-palette] .pal-svchead {
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 9px 14px;
  border-top: 1px solid color-mix(in srgb, var(--aurora-border-default) 40%, var(--aurora-page-bg));
  background: var(--gw0-0_38);
}
[data-palette] .pal-svcname {
  font-family: var(--font-display);
  font-size: 13px;
  font-weight: 760;
  color: var(--aurora-text-primary);
}
[data-palette] .pal-svcstatus {
  font-size: 10.5px;
  font-weight: 650;
}

/* ── Inline add-server flow ─────────────────────────────────────────────── */
[data-palette] .pal-add {
  padding: 14px 14px 12px;
  display: flex;
  flex-direction: column;
  gap: 12px;
  border-top: 1px solid color-mix(in srgb, var(--aurora-border-default) 30%, var(--aurora-page-bg));
}
[data-palette] .pal-add-cmd {
  display: flex;
  align-items: center;
  gap: 9px;
  height: 40px;
  padding: 0 4px 0 12px;
  border-radius: 11px;
  border: 1px solid color-mix(in srgb, var(--aurora-accent-primary) 32%, var(--aurora-border-strong));
  background: var(--gw4-0_62);
  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.03);
}
[data-palette] .pal-add-caret {
  flex-shrink: 0;
  color: var(--aurora-accent-strong);
  font-family: 'JetBrains Mono', var(--font-mono);
  font-size: 13px;
  font-weight: 700;
}
[data-palette] .pal-add-cmdinput {
  flex: 1;
  min-width: 0;
  height: 100%;
  border: none;
  background: none;
  outline: none;
  font-family: 'JetBrains Mono', var(--font-mono);
  font-size: 12px;
  color: var(--aurora-text-primary);
  caret-color: var(--aurora-accent-strong);
}
[data-palette] .pal-add-kind {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 20px;
  padding: 0 8px;
  margin-right: 6px;
  border-radius: 6px;
  border: 1px solid color-mix(in srgb, var(--aurora-accent-primary) 30%, transparent);
  background: color-mix(in srgb, var(--aurora-accent-primary) 10%, transparent);
  font-size: 8.5px;
  font-weight: 700;
  letter-spacing: 0.08em;
  color: var(--aurora-accent-strong);
}
[data-palette] .pal-add-grid {
  display: grid;
  grid-template-columns: 200px 1fr;
  gap: 12px;
  align-items: start;
}
[data-palette] .pal-add-col {
  display: flex;
  flex-direction: column;
  gap: 5px;
  min-width: 0;
}
[data-palette] .pal-add-label {
  font-size: 9.5px;
  font-weight: 700;
  letter-spacing: 0.13em;
  text-transform: uppercase;
  color: var(--aurora-text-muted);
}
[data-palette] .pal-add-input {
  height: 30px;
  padding: 0 10px;
  border-radius: 8px;
  border: 1px solid color-mix(in srgb, var(--aurora-border-default) 70%, var(--aurora-page-bg));
  background: var(--aurora-control-surface);
  outline: none;
  font-family: inherit;
  font-size: 12px;
  color: var(--aurora-text-primary);
}
[data-palette] .pal-add-input:focus {
  border-color: color-mix(in srgb, var(--aurora-accent-primary) 45%, transparent);
  box-shadow: 0 0 0 3px rgba(41, 182, 246, 0.14);
}
[data-palette] .pal-add-tokeninput {
  flex: 1;
  min-width: 80px;
  height: 24px;
  padding: 0 9px;
  border-radius: 7px;
  border: 1px solid color-mix(in srgb, var(--aurora-border-default) 70%, var(--aurora-page-bg));
  background: var(--aurora-control-surface);
  outline: none;
  font-family: inherit;
  font-size: 10.5px;
  color: var(--aurora-text-primary);
}
[data-palette] .pal-add-tokeninput:focus {
  border-color: color-mix(in srgb, var(--aurora-accent-primary) 45%, transparent);
}
[data-palette] .pal-add-pill {
  height: 24px;
  padding: 0 10px;
  border-radius: 7px;
  font-family: inherit;
  font-size: 10.5px;
  font-weight: 650;
  cursor: pointer;
  transition: background 150ms, border-color 150ms, color 150ms;
  border: 1px solid color-mix(in srgb, var(--aurora-border-default) 70%, var(--aurora-page-bg));
  background: var(--aurora-control-surface);
  color: var(--aurora-text-muted);
}
[data-palette] .pal-add-pill:hover {
  color: var(--aurora-text-primary);
  border-color: var(--aurora-border-strong);
}
[data-palette] .pal-add-pill[aria-pressed='true'] {
  border-color: color-mix(in srgb, var(--aurora-accent-primary) 45%, transparent);
  background: color-mix(in srgb, var(--aurora-accent-primary) 13%, transparent);
  color: var(--aurora-accent-strong);
}
[data-palette] .pal-add-note {
  height: 30px;
  display: inline-flex;
  align-items: center;
  font-size: 11px;
  color: color-mix(in srgb, var(--aurora-text-muted) 75%, transparent);
}
[data-palette] .pal-add-foot {
  display: flex;
  align-items: center;
  gap: 10px;
  padding-top: 10px;
  border-top: 1px solid color-mix(in srgb, var(--aurora-border-default) 30%, var(--aurora-page-bg));
}
[data-palette] .pal-switch {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 24px;
  padding: 0;
  border: none;
  background: none;
  font-family: inherit;
  cursor: pointer;
}
[data-palette] .pal-switch-track {
  width: 28px;
  height: 16px;
  flex-shrink: 0;
  border-radius: 999px;
  position: relative;
  transition: background 160ms;
  background: color-mix(in srgb, var(--aurora-border-strong) 80%, transparent);
}
[data-palette] .pal-switch[aria-checked='true'] .pal-switch-track {
  background: var(--aurora-accent-primary);
}
[data-palette] .pal-switch-thumb {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 12px;
  height: 12px;
  border-radius: 999px;
  background: #07131c;
  transition: left 160ms;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.4);
}
[data-palette] .pal-switch[aria-checked='true'] .pal-switch-thumb {
  left: 14px;
}
[data-palette] .pal-switch-label {
  font-size: 11px;
  font-weight: 650;
  color: var(--aurora-text-primary);
  white-space: nowrap;
}
[data-palette] .pal-btn {
  height: 28px;
  padding: 0 11px;
  border-radius: 8px;
  border: 1px solid color-mix(in srgb, var(--aurora-border-default) 70%, var(--aurora-page-bg));
  background: var(--aurora-control-surface);
  color: var(--aurora-text-muted);
  font-family: inherit;
  font-size: 11px;
  font-weight: 650;
  cursor: pointer;
  white-space: nowrap;
}
[data-palette] .pal-btn:hover {
  color: var(--aurora-text-primary);
  background: var(--aurora-hover-bg);
}
[data-palette] .pal-btn[data-primary='1'] {
  padding: 0 13px;
  border-color: color-mix(in srgb, var(--aurora-accent-primary) 55%, var(--aurora-border-strong));
  background: color-mix(in srgb, var(--aurora-accent-primary) 9%, var(--aurora-panel-strong));
  color: #bfe7fb;
}
[data-palette] .pal-btn[data-primary='1']:hover {
  box-shadow:
    0 0 0 1px color-mix(in srgb, var(--aurora-accent-primary) 34%, transparent),
    inset 0 1px 0 rgba(255, 255, 255, 0.07);
  background: color-mix(in srgb, var(--aurora-accent-primary) 13%, var(--aurora-panel-strong));
}
[data-palette] .pal-btn:disabled {
  opacity: 0.6;
  cursor: default;
}

/* ── Param prompt (no mock counterpart; matched to palette chrome) ──────── */
[data-palette] .pal-form {
  padding: 14px;
  display: flex;
  flex-direction: column;
  gap: 12px;
  border-top: 1px solid color-mix(in srgb, var(--aurora-border-default) 30%, var(--aurora-page-bg));
}

/* ── Footer ─────────────────────────────────────────────────────────────── */
[data-palette] .pal-foot {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 14px;
  border-top: 1px solid color-mix(in srgb, var(--aurora-border-default) 70%, var(--aurora-page-bg));
  background: var(--gw0-0_38);
  font-size: 11px;
  color: var(--aurora-text-muted);
  font-variant-numeric: tabular-nums;
}

/* ── Mobile dock (verbatim from the mock's max-width: 900px block) ──────── */
@media (max-width: 900px) {
  div[data-palette] {
    top: auto !important;
    bottom: 0 !important;
    left: 0 !important;
    right: 0 !important;
    margin: 0 !important;
    width: 100% !important;
    max-width: none !important;
    border-radius: 18px 18px 0 0 !important;
    max-height: 82vh;
    overflow-y: auto;
    padding-bottom: env(safe-area-inset-bottom);
  }
  [data-palette] [data-palrow] {
    min-height: 44px;
  }
  [data-palette] .pal-add-grid {
    grid-template-columns: 1fr;
  }
}

@media (prefers-reduced-motion: reduce) {
  [data-palette] [data-palrow],
  [data-palette] [data-palrow]::before {
    transition-duration: 1ms;
  }
}
`

/**
 * Injects the palette stylesheet. Rendered once, inside the palette portal.
 */
export function PaletteStyles() {
  return <style>{PALETTE_CSS}</style>
}
