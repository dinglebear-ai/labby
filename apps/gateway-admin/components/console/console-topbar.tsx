'use client'

import * as React from 'react'
import { Menu, Search } from 'lucide-react'

import { useConsoleShell } from '@/components/console/console-shell-context'
import { ConsoleStatusStrip } from '@/components/console/console-status-strip'
import { AccountMenu } from '@/components/console/console-sidebar'
import { OPEN_COMMAND_PALETTE_EVENT } from '@/lib/command-palette-events'

// Measured off the rendered mock, not inferred.
const SEARCH_WIDTH_IDLE = 'clamp(150px, 26vw, 340px)'
const SEARCH_WIDTH_HOVER = 'clamp(150px, 26vw, 340px)'

function isMacOS() {
  if (typeof navigator === 'undefined') return false
  return /mac/i.test(navigator.platform || navigator.userAgent)
}

/**
 * The console's single topbar: breadcrumb rail on the left, a centre-anchored
 * search pill in the right-hand flex flow, and an action cluster. Screens
 * fill the breadcrumb and action regions through `<AppHeader />`, which portals
 * into the slots registered here.
 */
export function ConsoleTopbar() {
  const { setCrumbSlot, setActionSlot, mobileNavOpen, toggleMobileNav } = useConsoleShell()
  const [searchHovered, setSearchHovered] = React.useState(false)
  const [modKey, setModKey] = React.useState('⌘')

  React.useEffect(() => {
    setModKey(isMacOS() ? '⌘' : 'Ctrl')
  }, [])

  const openPalette = React.useCallback(() => {
    window.dispatchEvent(new Event(OPEN_COMMAND_PALETTE_EVENT))
  }, [])

  return (
    <header
      data-topbar="1"
      style={{
        height: 56,
        boxSizing: 'border-box',
        flexShrink: 0,
        display: 'flex',
        alignItems: 'center',
        gap: 12,
        padding: '0 16px',
        borderBottom:
          '1px solid color-mix(in srgb, var(--aurora-border-default) 70%, var(--aurora-page-bg))',
        boxShadow: 'var(--aurora-shadow-medium), inset 0 1px 0 rgba(255,255,255,0.035)',
        background: 'color-mix(in srgb, var(--aurora-control-surface) 48%, transparent)',
        position: 'relative',
        zIndex: 40,
      }}
    >
      <button
        type="button"
        data-mobile-only="1"
        data-mobile-menu="1"
        aria-controls="console-navigation"
        aria-label={mobileNavOpen ? 'Close navigation' : 'Open navigation'}
        aria-expanded={mobileNavOpen}
        onClick={toggleMobileNav}
      >
        <Menu size={19} strokeWidth={1.8} />
      </button>
      <div
        ref={setCrumbSlot}
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 11,
          fontSize: 12.5,
          lineHeight: 'normal',
          minWidth: 0,
        }}
      />
      <ConsoleStatusStrip />

      <div style={{ flex: 1 }} />

      <button
        type="button"
        data-searchbar="1"
        onClick={openPalette}
        aria-label="Search and filter"
        onMouseEnter={() => setSearchHovered(true)}
        onMouseLeave={() => setSearchHovered(false)}
        style={{
          flex: '0 1 auto',
          maxWidth: 340,
          display: 'flex',
          alignItems: 'center',
          gap: 8,
          height: 36,
          width: searchHovered ? SEARCH_WIDTH_HOVER : SEARCH_WIDTH_IDLE,
          transition:
            'width 200ms ease-out, border-color 200ms, box-shadow 250ms, color 200ms',
          minWidth: 0,
          whiteSpace: 'nowrap',
          overflow: 'hidden',
          padding: '0 11px',
          borderRadius: 999,
          border: `1px solid ${
            searchHovered
              ? 'color-mix(in srgb, var(--aurora-accent-primary) 40%, var(--aurora-border-strong))'
              : 'color-mix(in srgb, var(--aurora-border-default) 70%, var(--aurora-page-bg))'
          }`,
          background:
            'linear-gradient(180deg, rgba(255,255,255,0.025), transparent), var(--aurora-control-surface)',
          color: searchHovered ? 'var(--aurora-text-primary)' : 'var(--aurora-text-muted)',
          fontFamily: 'inherit',
          fontSize: 12.5,
          lineHeight: 'normal',
          marginRight: 34,
          cursor: 'pointer',
          boxShadow: searchHovered
            ? '0 0 0 3px rgba(41,182,246,0.09), 0 0 16px rgba(41,182,246,0.10), inset 0 1px 0 rgba(255,255,255,0.05)'
            : 'inset 0 1px 0 rgba(255,255,255,0.03)',
        }}
      >
        <Search
          size={14}
          strokeWidth={1.6}
          style={{
            flexShrink: 0,
            color: searchHovered
              ? 'var(--aurora-accent-strong)'
              : 'var(--aurora-text-muted)',
            transition: 'color 200ms',
          }}
        />
        <span
          data-search-label="1"
          style={{
            flex: 1,
            textAlign: 'left',
            whiteSpace: 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
          }}
        >
          Search
        </span>
        <span
          data-search-shortcut="1"
          className="max-[520px]:!hidden"
          aria-hidden="true"
          style={{
            flexShrink: 0,
            display: 'inline-flex',
            alignItems: 'center',
            height: 20,
            padding: '0 5px',
            marginRight: -3,
            borderRadius: 5,
            border: '1px solid color-mix(in srgb, var(--aurora-border-strong) 70%, var(--aurora-page-bg))',
            background: 'color-mix(in srgb, var(--aurora-page-bg) 38%, transparent)',
            color: 'var(--aurora-text-muted)',
            fontSize: 10,
            lineHeight: 'normal',
            fontWeight: 650,
          }}
        >
          {modKey}K
        </span>
      </button>

      <div
        ref={setActionSlot}
        data-actioncluster="1"
        style={{
          flexShrink: 0,
          display: 'flex',
          alignItems: 'center',
          gap: 5,
        }}
      />
      <AccountMenu placement="topbar" />
    </header>
  )
}
