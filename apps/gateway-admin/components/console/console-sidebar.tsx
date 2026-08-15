'use client'

import * as React from 'react'
import Link from 'next/link'
import { useRouter, usePathname } from 'next/navigation'
import {
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  ChevronsUpDown,
  LogOut,
  Moon,
  Palette,
  Pin,
  ScrollText,
  Settings,
  Sun,
} from 'lucide-react'
import { useTheme } from 'next-themes'

import { LabbyIcon } from '@/components/labby-icon'
import { useConsoleShell } from '@/components/console/console-shell-context'
import {
  consoleNavSections,
  isNavItemActive,
  type ConsoleNavItem,
} from '@/components/console/nav-model'
import {
  sessionAvatarFallback,
  sessionPrimaryEmail,
} from '@/lib/auth/session-presenter'
import { logoutBrowserSession, useBrowserSession } from '@/lib/auth/session'

const PINNED_KEY = 'labby-nav-pinned'
const FOLDED_KEY = 'labby-nav-folded'
const ORDER_KEY = 'labby-nav-order-v2'

// Measured off the rendered mock (`Gateway Console.dc.html`), not inferred.
const SIDEBAR_WIDTH_EXPANDED = '224px'
const SIDEBAR_WIDTH_COLLAPSED = '58px'

/** The sidebar's own tinted plate — the mock lifts it off the page background. */
const SIDEBAR_BG = 'color-mix(in srgb, #0f2334 48%, transparent)'
/** Ring colour for status pips, matched to the sidebar plate rather than the page. */
const PIP_RING = 'color-mix(in srgb, #0f2334 80%, var(--aurora-page-bg))'

function readJson<T>(key: string, fallback: T): T {
  try {
    const raw = window.localStorage.getItem(key)
    if (!raw) return fallback
    return JSON.parse(raw) as T
  } catch {
    return fallback
  }
}

function writeJson(key: string, value: unknown) {
  try {
    window.localStorage.setItem(key, JSON.stringify(value))
  } catch {
    /* storage unavailable — the preference is simply not persisted */
  }
}

// ── Nav item ──────────────────────────────────────────────────────────────────

type NavItemProps = {
  item: ConsoleNavItem
  sectionId: string
  active: boolean
  collapsed: boolean
  pinned: boolean
  onTogglePin: (id: string) => void
  onDragStart: (id: string) => void
  onDropOn: (id: string) => void
}

function NavItem({
  item,
  sectionId,
  active,
  collapsed,
  pinned,
  onTogglePin,
  onDragStart,
  onDropOn,
}: NavItemProps) {
  const [hovered, setHovered] = React.useState(false)
  const Icon = item.icon

  return (
    <Link
      href={item.href}
      data-navitem="1"
      aria-current={active ? 'true' : 'false'}
      data-tip={item.tooltip}
      title={collapsed ? '' : item.tooltip}
      draggable
      onDragStart={() => onDragStart(item.id)}
      onDragOver={(event) => event.preventDefault()}
      onDrop={(event) => {
        event.preventDefault()
        onDropOn(item.id)
      }}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 10,
        width: '100%',
        minHeight: 34,
        padding: '3px 10px',
        borderRadius: 10,
        borderWidth: 1,
        borderStyle: 'solid',
        borderColor: active
          ? 'color-mix(in srgb, var(--aurora-accent-primary) 26%, transparent)'
          : 'transparent',
        background: active
          ? 'color-mix(in srgb, var(--aurora-accent-primary) 12%, transparent)'
          : hovered
            ? 'var(--aurora-hover-bg)'
            : 'none',
        boxShadow: active ? 'inset 0 1px 0 rgba(255,255,255,0.04)' : undefined,
        fontFamily: 'inherit',
        fontSize: 13,
        fontWeight: 560,
        color:
          active || hovered
            ? 'var(--aurora-text-primary)'
            : 'var(--aurora-text-muted)',
        textAlign: 'left',
        whiteSpace: 'nowrap',
        cursor: 'pointer',
        textDecoration: 'none',
        transition: 'background 150ms, color 150ms',
      }}
    >
      <span
        style={{
          position: 'relative',
          flexShrink: 0,
          display: 'grid',
          placeItems: 'center',
          width: 18,
          height: 18,
        }}
      >
        <Icon size={16} strokeWidth={1.8} />
      </span>

      {collapsed ? null : (
        <>
          <span
            data-anim="navlabel"
            style={{
              flex: 1,
              minWidth: 0,
              display: 'flex',
              flexDirection: 'column',
              justifyContent: 'center',
              overflow: 'hidden',
            }}
          >
            <span
              style={{
                whiteSpace: 'nowrap',
                overflow: 'hidden',
                textOverflow: 'ellipsis',
              }}
            >
              {item.label}
            </span>
            {active && item.contextLine ? (
              <span
                style={{
                  fontSize: 9.5,
                  lineHeight: 1.4,
                  color: 'color-mix(in srgb, var(--aurora-text-muted) 80%, transparent)',
                  whiteSpace: 'nowrap',
                  overflow: 'hidden',
                  textOverflow: 'ellipsis',
                  fontVariantNumeric: 'tabular-nums',
                }}
              >
                {item.contextLine}
              </span>
            ) : null}
          </span>

          <span
            data-pinbtn="1"
            data-pinned={pinned ? '1' : '0'}
            role="button"
            tabIndex={0}
            aria-label={pinned ? 'Unpin' : `Pin to top of ${sectionId}`}
            title={pinned ? 'Unpin' : `Pin to top of ${sectionId}`}
            onClick={(event) => {
              event.preventDefault()
              event.stopPropagation()
              onTogglePin(item.id)
            }}
            onKeyDown={(event) => {
              if (event.key !== 'Enter' && event.key !== ' ') return
              event.preventDefault()
              event.stopPropagation()
              onTogglePin(item.id)
            }}
            style={{
              display: 'inline-flex',
              alignItems: 'center',
              justifyContent: 'center',
              width: 18,
              height: 18,
              borderRadius: 5,
              flexShrink: 0,
              cursor: 'pointer',
              color: pinned ? 'var(--aurora-accent-strong)' : 'var(--aurora-text-muted)',
            }}
          >
            <Pin
              size={11}
              strokeWidth={1.7}
              fill={
                pinned
                  ? 'color-mix(in srgb, var(--aurora-accent-primary) 40%, transparent)'
                  : 'none'
              }
            />
          </span>

          <span
            data-kbd="1"
            style={{
              flexShrink: 0,
              fontSize: 10,
              color: 'color-mix(in srgb, var(--aurora-text-muted) 65%, transparent)',
              fontVariantNumeric: 'tabular-nums',
            }}
          >
            {item.kbd}
          </span>
        </>
      )}
    </Link>
  )
}

// ── Account card ──────────────────────────────────────────────────────────────

function AccountCard({ collapsed }: { collapsed: boolean }) {
  const session = useBrowserSession()
  const [open, setOpen] = React.useState(false)
  const [hovered, setHovered] = React.useState(false)
  const [signingOut, setSigningOut] = React.useState(false)
  const { resolvedTheme, setTheme } = useTheme()
  const [mounted, setMounted] = React.useState(false)
  const rootRef = React.useRef<HTMLDivElement>(null)

  React.useEffect(() => setMounted(true), [])

  React.useEffect(() => {
    if (!open) return
    const onPointerDown = (event: MouseEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false)
    }
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false)
    }
    window.addEventListener('mousedown', onPointerDown)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('mousedown', onPointerDown)
      window.removeEventListener('keydown', onKey)
    }
  }, [open])

  const user = session.status === 'authenticated' ? session.user : null
  const email = user ? sessionPrimaryEmail(user) : 'Not signed in'
  const initial = user ? sessionAvatarFallback(user) : '?'
  const name = user ? email.split('@')[0] : 'Anonymous'
  const isDark = !mounted || resolvedTheme !== 'light'

  const signOut = async () => {
    setSigningOut(true)
    try {
      await logoutBrowserSession()
    } finally {
      setSigningOut(false)
    }
  }

  const menuRowStyle: React.CSSProperties = {
    display: 'flex',
    alignItems: 'center',
    gap: 9,
    width: '100%',
    height: 32,
    padding: '0 9px',
    borderRadius: 8,
    border: 'none',
    background: 'none',
    fontFamily: 'inherit',
    fontSize: 12.5,
    fontWeight: 560,
    color: 'var(--aurora-text-muted)',
    cursor: 'pointer',
    textAlign: 'left',
  }

  return (
    <div
      ref={rootRef}
      data-accountmenu="1"
      style={{ padding: '10px 10px 12px', minWidth: 0, position: 'relative' }}
    >
      {open ? (
        <div
          data-anim="menu"
          style={{
            position: 'fixed',
            bottom: 64,
            left: 10,
            width: 236,
            zIndex: 70,
            borderRadius: 'var(--radius-2)',
            border:
              '1px solid color-mix(in srgb, var(--aurora-border-default) 45%, var(--aurora-page-bg))',
            background:
              'linear-gradient(180deg, var(--aurora-panel-strong-top), var(--aurora-panel-strong))',
            boxShadow: 'var(--aurora-shadow-strong), inset 0 1px 0 rgba(255,255,255,0.05)',
            overflow: 'hidden',
          }}
        >
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 10,
              padding: '12px 13px',
              borderBottom:
                '1px solid color-mix(in srgb, var(--aurora-border-default) 60%, var(--aurora-page-bg))',
              background: 'var(--gw0-0_36)',
            }}
          >
            <div
              style={{
                width: 32,
                height: 32,
                flexShrink: 0,
                borderRadius: 999,
                display: 'grid',
                placeItems: 'center',
                background:
                  'color-mix(in srgb, var(--aurora-accent-primary) 16%, var(--aurora-panel-medium))',
                border:
                  '1px solid color-mix(in srgb, var(--aurora-accent-primary) 30%, transparent)',
                fontSize: 11,
                fontWeight: 700,
                color: 'var(--aurora-accent-strong)',
              }}
            >
              {initial}
            </div>
            <div style={{ minWidth: 0, lineHeight: 1.3 }}>
              <div
                style={{
                  fontSize: 12,
                  fontWeight: 650,
                  color: 'var(--aurora-text-primary)',
                  overflow: 'hidden',
                  textOverflow: 'ellipsis',
                  whiteSpace: 'nowrap',
                }}
              >
                {name}
              </div>
              <div
                style={{
                  fontSize: 10.5,
                  color: 'var(--aurora-text-muted)',
                  overflow: 'hidden',
                  textOverflow: 'ellipsis',
                  whiteSpace: 'nowrap',
                }}
              >
                {email}
              </div>
            </div>
          </div>

          <div style={{ padding: 5 }}>
            <button
              type="button"
              data-menurow="1"
              onClick={() => setTheme(isDark ? 'light' : 'dark')}
              style={menuRowStyle}
            >
              <span
                style={{
                  flexShrink: 0,
                  display: 'grid',
                  placeItems: 'center',
                  width: 16,
                  height: 16,
                }}
              >
                {isDark ? <Sun size={14} strokeWidth={1.7} /> : <Moon size={14} strokeWidth={1.7} />}
              </span>
              <span style={{ flex: 1, whiteSpace: 'nowrap' }}>Appearance</span>
              <span style={{ fontSize: 10.5, color: 'var(--aurora-text-muted)' }}>
                {isDark ? 'Dark' : 'Light'}
              </span>
            </button>
            {/* Docs and the Aurora gallery are reference surfaces, not console
                destinations — the mock keeps its nav to the four working
                sections, so these live here rather than in the sidebar list. */}
            <Link
              href="/docs"
              data-menurow="1"
              onClick={() => setOpen(false)}
              style={{ ...menuRowStyle, textDecoration: 'none' }}
            >
              <span
                style={{
                  flexShrink: 0,
                  display: 'grid',
                  placeItems: 'center',
                  width: 16,
                  height: 16,
                }}
              >
                <ScrollText size={14} strokeWidth={1.7} />
              </span>
              <span style={{ flex: 1, whiteSpace: 'nowrap' }}>Documentation</span>
            </Link>
            <Link
              href="/design-system"
              data-menurow="1"
              onClick={() => setOpen(false)}
              style={{ ...menuRowStyle, textDecoration: 'none' }}
            >
              <span
                style={{
                  flexShrink: 0,
                  display: 'grid',
                  placeItems: 'center',
                  width: 16,
                  height: 16,
                }}
              >
                <Palette size={14} strokeWidth={1.7} />
              </span>
              <span style={{ flex: 1, whiteSpace: 'nowrap' }}>Design System</span>
            </Link>
          </div>

          {user ? (
            <div
              style={{
                padding: 5,
                borderTop:
                  '1px solid color-mix(in srgb, var(--aurora-border-default) 60%, var(--aurora-page-bg))',
              }}
            >
              <button
                type="button"
                data-menurow="1"
                disabled={signingOut}
                onClick={() => void signOut()}
                style={menuRowStyle}
              >
                <LogOut size={14} strokeWidth={1.7} />
                {signingOut ? 'Signing out…' : 'Sign Out'}
              </button>
            </div>
          ) : null}
        </div>
      ) : null}

      <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        aria-expanded={open}
        aria-label="Account menu"
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 9,
          width: '100%',
          padding: '7px 8px',
          borderRadius: 'var(--radius-1)',
          border: `1px solid ${
            hovered
              ? 'var(--aurora-border-strong)'
              : 'color-mix(in srgb, var(--aurora-border-default) 55%, var(--aurora-page-bg))'
          }`,
          background: hovered
            ? 'var(--aurora-hover-bg)'
            : 'linear-gradient(180deg, var(--aurora-panel-medium-top), transparent), color-mix(in srgb, var(--aurora-panel-medium) 55%, var(--aurora-nav-bg))',
          boxShadow: 'inset 0 1px 0 rgba(255,255,255,0.035)',
          fontFamily: 'inherit',
          cursor: 'pointer',
          minWidth: 0,
          justifyContent: collapsed ? 'center' : undefined,
          transition: 'border-color 150ms, background 150ms',
        }}
      >
        <div
          title={
            session.status === 'authenticated'
              ? 'Session active'
              : 'No active browser session'
          }
          style={{
            position: 'relative',
            width: 30,
            height: 30,
            flexShrink: 0,
            borderRadius: 999,
            display: 'grid',
            placeItems: 'center',
            background:
              'color-mix(in srgb, var(--aurora-accent-primary) 16%, var(--aurora-panel-medium))',
            border: '1px solid color-mix(in srgb, var(--aurora-accent-primary) 30%, transparent)',
            fontSize: 11,
            fontWeight: 700,
            color: 'var(--aurora-accent-strong)',
          }}
        >
          {initial}
          <span
            style={{
              position: 'absolute',
              right: -1,
              bottom: -1,
              width: 8,
              height: 8,
              borderRadius: 999,
              background: user ? 'var(--aurora-success)' : 'var(--aurora-warn)',
              boxShadow: `0 0 4px ${user ? 'var(--aurora-success)' : 'var(--aurora-warn)'}, 0 0 0 2px ${PIP_RING}`,
            }}
          />
        </div>

        {collapsed ? null : (
          <>
            <div style={{ minWidth: 0, flex: 1, lineHeight: 1.3, textAlign: 'left' }}>
              <div
                style={{
                  fontSize: 12,
                  fontWeight: 650,
                  color: 'var(--aurora-text-primary)',
                  overflow: 'hidden',
                  textOverflow: 'ellipsis',
                  whiteSpace: 'nowrap',
                }}
              >
                {name}
              </div>
              <div
                style={{
                  fontSize: 10.5,
                  color: 'var(--aurora-text-muted)',
                  overflow: 'hidden',
                  textOverflow: 'ellipsis',
                  whiteSpace: 'nowrap',
                }}
              >
                {email}
              </div>
            </div>
            <span
              title="Gateway environment"
              style={{
                display: 'inline-flex',
                alignItems: 'center',
                gap: 4,
                height: 16,
                padding: '0 6px',
                borderRadius: 4,
                fontSize: 8,
                fontWeight: 700,
                letterSpacing: '0.1em',
                color: 'var(--aurora-success)',
                background: 'color-mix(in srgb, var(--aurora-success) 11%, transparent)',
                border: '1px solid color-mix(in srgb, var(--aurora-success) 30%, transparent)',
                flexShrink: 0,
              }}
            >
              <span
                style={{
                  width: 4,
                  height: 4,
                  borderRadius: 999,
                  background: 'currentColor',
                  boxShadow: '0 0 4px currentColor',
                }}
              />
              PROD
            </span>
            <ChevronsUpDown
              size={13}
              strokeWidth={1.7}
              style={{ flexShrink: 0, color: 'var(--aurora-text-muted)' }}
            />
          </>
        )}
      </button>
    </div>
  )
}

// ── Sidebar ───────────────────────────────────────────────────────────────────

export function ConsoleSidebar() {
  const pathname = usePathname()
  const router = useRouter()
  const { collapsed, toggleCollapsed } = useConsoleShell()

  const [pinned, setPinned] = React.useState<string[]>([])
  const [folded, setFolded] = React.useState<Record<string, boolean>>({})
  const [order, setOrder] = React.useState<Record<string, string[]>>({})
  const [toggleHovered, setToggleHovered] = React.useState(false)
  const dragRef = React.useRef<{ section: string; id: string } | null>(null)

  React.useEffect(() => {
    setPinned(readJson<string[]>(PINNED_KEY, []))
    setFolded(readJson<Record<string, boolean>>(FOLDED_KEY, {}))
    setOrder(readJson<Record<string, string[]>>(ORDER_KEY, {}))
  }, [])

  // ⌘/Ctrl + N jumps to the Nth nav item, matching the mock's accelerators.
  React.useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!event.metaKey && !event.ctrlKey) return
      if (event.altKey || event.shiftKey) return
      const index = Number.parseInt(event.key, 10)
      if (Number.isNaN(index) || index < 1) return
      const flat = consoleNavSections.flatMap((section) => section.items)
      const target = flat[index - 1]
      if (!target) return
      event.preventDefault()
      router.push(target.href)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [router])

  const togglePin = React.useCallback((id: string) => {
    setPinned((current) => {
      const next = current.includes(id)
        ? current.filter((value) => value !== id)
        : [...current, id]
      writeJson(PINNED_KEY, next)
      return next
    })
  }, [])

  const toggleFold = React.useCallback((sectionId: string) => {
    setFolded((current) => {
      const next = { ...current, [sectionId]: !current[sectionId] }
      writeJson(FOLDED_KEY, next)
      return next
    })
  }, [])

  const orderedItems = React.useCallback(
    (section: (typeof consoleNavSections)[number]) => {
      const ids = section.items.map((item) => item.id)
      const saved = (order[section.id] ?? []).filter((id) => ids.includes(id))
      const sequence = [...saved, ...ids.filter((id) => !saved.includes(id))]
      const byId = new Map(section.items.map((item) => [item.id, item]))
      const resolved = sequence
        .map((id) => byId.get(id))
        .filter((item): item is ConsoleNavItem => Boolean(item))
      // Pinned items float to the top of their own section.
      return [
        ...resolved.filter((item) => pinned.includes(item.id)),
        ...resolved.filter((item) => !pinned.includes(item.id)),
      ]
    },
    [order, pinned],
  )

  const handleDrop = React.useCallback(
    (sectionId: string, targetId: string) => {
      const drag = dragRef.current
      dragRef.current = null
      if (!drag || drag.section !== sectionId || drag.id === targetId) return
      const section = consoleNavSections.find((entry) => entry.id === sectionId)
      if (!section) return
      const ids = section.items.map((item) => item.id)
      const saved = (order[sectionId] ?? []).filter((id) => ids.includes(id))
      const sequence = [...saved, ...ids.filter((id) => !saved.includes(id))].filter(
        (id) => id !== drag.id,
      )
      sequence.splice(sequence.indexOf(targetId), 0, drag.id)
      const next = { ...order, [sectionId]: sequence }
      writeJson(ORDER_KEY, next)
      setOrder(next)
    },
    [order],
  )

  const settingsActive = isNavItemActive('/settings', pathname)

  return (
    <aside
      data-console-sidebar="1"
      style={{
        position: 'relative',
        width: collapsed ? SIDEBAR_WIDTH_COLLAPSED : SIDEBAR_WIDTH_EXPANDED,
        flexShrink: 0,
        display: 'flex',
        flexDirection: 'column',
        background: SIDEBAR_BG,
        transition: 'width 240ms cubic-bezier(0.2,0.8,0.2,1)',
      }}
    >
      <span
        aria-hidden
        style={{
          position: 'absolute',
          top: 56,
          right: 0,
          bottom: 0,
          width: 1,
          background:
            'color-mix(in srgb, var(--aurora-border-default) 60%, var(--aurora-page-bg))',
          pointerEvents: 'none',
          zIndex: 5,
        }}
      />

      <button
        type="button"
        onClick={toggleCollapsed}
        aria-label="Toggle sidebar"
        title="Toggle sidebar"
        onMouseEnter={() => setToggleHovered(true)}
        onMouseLeave={() => setToggleHovered(false)}
        style={{
          position: 'absolute',
          top: '50%',
          transform: 'translateY(-50%)',
          right: -11,
          zIndex: 10,
          width: 22,
          height: 22,
          borderRadius: 999,
          border: `1px solid ${
            toggleHovered
              ? 'color-mix(in srgb, var(--aurora-accent-primary) 40%, var(--aurora-border-strong))'
              : 'color-mix(in srgb, var(--aurora-border-strong) 80%, var(--aurora-page-bg))'
          }`,
          background: 'var(--aurora-panel-medium)',
          color: toggleHovered ? 'var(--aurora-text-primary)' : 'var(--aurora-text-muted)',
          display: 'grid',
          placeItems: 'center',
          cursor: 'pointer',
          boxShadow: '0 2px 6px rgba(0,0,0,0.3)',
        }}
      >
        {collapsed ? (
          <ChevronRight size={13} strokeWidth={2} />
        ) : (
          <ChevronLeft size={13} strokeWidth={2} />
        )}
      </button>

      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          height: '100%',
          overflow: 'visible',
        }}
      >
        {/* Brand */}
        <Link
          href="/"
          aria-label="Go to Overview"
          title="Labby — gateway control plane"
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 10,
            height: 56,
            boxSizing: 'border-box',
            flexShrink: 0,
            padding: '0 14px',
            borderBottom:
              '1px solid color-mix(in srgb, var(--aurora-border-default) 70%, var(--aurora-page-bg))',
            minWidth: 0,
            width: '100%',
            textDecoration: 'none',
            color: 'var(--aurora-text-primary)',
          }}
        >
          <div
            style={{
              position: 'relative',
              width: 34,
              height: 34,
              flexShrink: 0,
              display: 'grid',
              placeItems: 'center',
            }}
          >
            <LabbyIcon size={34} />
          </div>
          {collapsed ? null : (
            <div style={{ minWidth: 0, display: 'flex', alignItems: 'center', gap: 7 }}>
              <div
                style={{
                  fontFamily: 'var(--font-display)',
                  fontWeight: 800,
                  fontSize: 15,
                  letterSpacing: '0.01em',
                  whiteSpace: 'nowrap',
                }}
              >
                Lab<span style={{ color: 'var(--aurora-accent-strong)' }}>by</span>
              </div>
              <span
                style={{
                  display: 'inline-flex',
                  alignItems: 'center',
                  height: 15,
                  padding: '0 5px',
                  borderRadius: 4,
                  border:
                    '1px solid color-mix(in srgb, var(--aurora-accent-primary) 30%, transparent)',
                  background: 'color-mix(in srgb, var(--aurora-accent-primary) 9%, transparent)',
                  fontSize: 8,
                  fontWeight: 700,
                  letterSpacing: '0.12em',
                  color: 'var(--aurora-accent-strong)',
                }}
              >
                LAB
              </span>
            </div>
          )}
        </Link>

        {/* Nav */}
        <nav
          data-collapsed={collapsed ? '1' : '0'}
          style={{
            flex: 1,
            display: 'flex',
            flexDirection: 'column',
            gap: 2,
            padding: '8px 8px 0',
            minWidth: 0,
            minHeight: 0,
            overflowY: 'auto',
            overflowX: 'visible',
          }}
        >
          {consoleNavSections.map((section) => {
            const isFolded = Boolean(folded[section.id])
            const items = orderedItems(section)
            const hasActive = items.some((item) => isNavItemActive(item.href, pathname))

            return (
              <React.Fragment key={section.id}>
                {collapsed ? null : (
                  <button
                    type="button"
                    onClick={() => toggleFold(section.id)}
                    aria-expanded={!isFolded}
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      gap: 6,
                      width: '100%',
                      border: 'none',
                      background: 'none',
                      cursor: 'pointer',
                      padding: '9px 8px 4px',
                      fontFamily: 'inherit',
                      fontSize: 9.5,
                      fontWeight: 700,
                      letterSpacing: '0.11em',
                      textTransform: 'uppercase',
                      color: 'color-mix(in srgb, var(--aurora-text-muted) 70%, transparent)',
                      textAlign: 'left',
                      transition: 'color 150ms ease-out',
                    }}
                  >
                    <ChevronDown
                      size={10}
                      strokeWidth={2.2}
                      style={{
                        transform: isFolded ? 'rotate(-90deg)' : 'none',
                        transition: 'transform 200ms ease-out',
                        flexShrink: 0,
                      }}
                    />
                    <span
                      style={{
                        flex: 1,
                        whiteSpace: 'nowrap',
                        overflow: 'hidden',
                        textOverflow: 'ellipsis',
                      }}
                    >
                      {section.label}
                    </span>
                  </button>
                )}

                {isFolded && !collapsed ? null : (
                  <div
                    style={{
                      position: 'relative',
                      display: 'flex',
                      flexDirection: 'column',
                      gap: 2,
                      paddingLeft: collapsed ? 0 : 8,
                    }}
                  >
                    {hasActive && !collapsed ? (
                      <span
                        aria-hidden
                        style={{
                          position: 'absolute',
                          left: 2,
                          top: 4,
                          bottom: 4,
                          width: 2,
                          borderRadius: 999,
                          background:
                            'color-mix(in srgb, var(--aurora-accent-primary) 75%, transparent)',
                        }}
                      />
                    ) : null}
                    {items.map((item) => (
                      <NavItem
                        key={item.id}
                        item={item}
                        sectionId={section.id}
                        active={isNavItemActive(item.href, pathname)}
                        collapsed={collapsed}
                        pinned={pinned.includes(item.id)}
                        onTogglePin={togglePin}
                        onDragStart={(id) => {
                          dragRef.current = { section: section.id, id }
                        }}
                        onDropOn={(id) => handleDrop(section.id, id)}
                      />
                    ))}
                  </div>
                )}
              </React.Fragment>
            )
          })}

          <div style={{ flex: 1 }} />

          <Link
            href="/settings"
            data-navitem="1"
            data-tip="Settings"
            aria-current={settingsActive ? 'true' : 'false'}
            title={collapsed ? '' : 'Settings'}
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 10,
              width: '100%',
              minHeight: 34,
              margin: '4px 0 8px',
              padding: '3px 10px',
              borderRadius: 10,
              borderWidth: 1,
              borderStyle: 'solid',
              borderColor: settingsActive
                ? 'color-mix(in srgb, var(--aurora-accent-primary) 26%, transparent)'
                : 'transparent',
              background: settingsActive
                ? 'color-mix(in srgb, var(--aurora-accent-primary) 12%, transparent)'
                : 'none',
              boxShadow: settingsActive ? 'inset 0 1px 0 rgba(255,255,255,0.04)' : undefined,
              fontFamily: 'inherit',
              fontSize: 13,
              fontWeight: 560,
              color: settingsActive
                ? 'var(--aurora-text-primary)'
                : 'var(--aurora-text-muted)',
              textDecoration: 'none',
              whiteSpace: 'nowrap',
              cursor: 'pointer',
              transition: 'background 150ms, color 150ms',
            }}
          >
            <span
              style={{
                flexShrink: 0,
                display: 'grid',
                placeItems: 'center',
                width: 18,
                height: 18,
              }}
            >
              <Settings size={16} strokeWidth={1.8} />
            </span>
            {collapsed ? null : <span style={{ whiteSpace: 'nowrap' }}>Settings</span>}
          </Link>
        </nav>

        <AccountCard collapsed={collapsed} />
      </div>
    </aside>
  )
}
