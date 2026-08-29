'use client'

import * as React from 'react'
import { useRouter } from 'next/navigation'
import { Bell, MoreHorizontal, Plus, Search } from 'lucide-react'

import { useConsoleShell } from '@/components/console/console-shell-context'
import { AccountCard } from '@/components/console/console-sidebar'
import {
  OPEN_ADD_SERVER_PALETTE_EVENT,
  OPEN_COMMAND_PALETTE_EVENT,
} from '@/lib/command-palette-events'

// Measured off the rendered mock, not inferred.
const SEARCH_WIDTH_IDLE = 'clamp(120px, 22vw, 300px)'
const SEARCH_WIDTH_HOVER = 'clamp(150px, 26vw, 340px)'

function isMacOS() {
  if (typeof navigator === 'undefined') return false
  return /mac/i.test(navigator.platform || navigator.userAgent)
}

/**
 * The console's single topbar: breadcrumb rail on the left, a centre-anchored
 * search pill that widens on hover, and a right-hand action cluster. Screens
 * fill the breadcrumb and action regions through `<AppHeader />`, which portals
 * into the slots registered here.
 */
export function ConsoleTopbar() {
  const router = useRouter()
  const { setCrumbSlot, setActionSlot } = useConsoleShell()
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
        background: 'color-mix(in srgb, #0f2334 48%, transparent)',
        position: 'relative',
        zIndex: 40,
      }}
    >
      <div
        ref={setCrumbSlot}
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 7,
          fontSize: 13,
          minWidth: 0,
        }}
      />

      <div style={{ flex: 1 }} />

      <button
        type="button"
        data-searchbar="1"
        onClick={openPalette}
        aria-label="Search and filter"
        onMouseEnter={() => setSearchHovered(true)}
        onMouseLeave={() => setSearchHovered(false)}
        style={{
          position: 'absolute',
          left: '50%',
          transform: 'translateX(-50%)',
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
          style={{
            flex: 1,
            textAlign: 'left',
            whiteSpace: 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
          }}
        >
          Search — {modKey}K
        </span>
        <span
          title="Notifications"
          style={{
            position: 'relative',
            flexShrink: 0,
            display: 'grid',
            placeItems: 'center',
            width: 24,
            height: 24,
            marginRight: -4,
            borderRadius: 999,
            color: 'var(--aurora-text-muted)',
          }}
        >
          <Bell size={13} strokeWidth={1.7} />
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

      <div className="flex shrink-0 items-center gap-1">
        <button
          type="button"
          onClick={() => router.push('/gateways/?add=1')}
          className="inline-flex h-8 items-center gap-1.5 rounded-l-[9px] border border-aurora-accent-primary/35 bg-aurora-accent-primary/10 px-3 text-[11px] font-semibold text-aurora-accent-strong transition-colors hover:bg-aurora-accent-primary/16 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary/35"
        >
          <Plus className="size-3.5" />
          <span className="hidden sm:inline">Add Server</span>
        </button>
        <button
          type="button"
          aria-label="More add options"
          title="Open inline Add Server options"
          onClick={() => window.dispatchEvent(new Event(OPEN_ADD_SERVER_PALETTE_EVENT))}
          className="grid size-8 place-items-center rounded-r-[9px] border border-l-0 border-aurora-accent-primary/35 bg-aurora-accent-primary/10 text-aurora-accent-strong transition-colors hover:bg-aurora-accent-primary/16 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary/35"
        >
          <MoreHorizontal className="size-3.5" />
        </button>
      </div>

      <AccountCard placement="topbar" />
    </header>
  )
}
