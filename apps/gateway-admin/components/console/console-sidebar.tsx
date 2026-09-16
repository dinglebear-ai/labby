'use client'

import * as React from 'react'
import Link from 'next/link'
import { useRouter, usePathname } from 'next/navigation'
import {
  ChevronDown,
  ChevronsUpDown,
  Check,
  ChevronLeft,
  ChevronRight,
  LogOut,
  Moon,
  Pin,
  Settings,
  Sun,
} from 'lucide-react'
import { useTheme } from 'next-themes'
import { toast } from 'sonner'

import { useConsoleStatus } from '@/components/console/console-status-strip'
import { getStats, StashError } from '@/lib/stash/client'
import { LabbyIcon } from '@/components/labby-icon'
import { useConsoleShell } from '@/components/console/console-shell-context'
import {
  capabilityAwareNavSections,
  consoleNavSections,
  isNavItemActive,
  type ConsoleNavItem,
} from '@/components/console/nav-model'
import { sessionPrimaryEmail } from '@/lib/auth/session-presenter'
import { LogoutRevocationError, WorkspaceSelectionError, logoutBrowserSession, selectSessionWorkspace, useBrowserSession } from '@/lib/auth/session'

/**
 * Switch the active workspace and report a rejected selection instead of
 * letting it escape as an unhandled promise rejection or a render error.
 * Returns whether the switch happened so callers can decide about navigation.
 */
function switchWorkspace(selection: { teamId?: string | null; projectId?: string | null }): boolean {
  try {
    selectSessionWorkspace(selection)
    return true
  } catch (error) {
    if (!(error instanceof WorkspaceSelectionError)) throw error
    toast.error(error.message)
    return false
  }
}

const PINNED_KEY = 'labby-nav-pinned'
const FOLDED_KEY = 'labby-nav-folded'
const ORDER_KEY = 'labby-nav-order-v2'
const SECTION_ORDER_KEY = 'labby-nav-sections-v4'
const SECTION_IDS = consoleNavSections.map(section => section.id)

// Measured off the rendered mock (`Gateway Console.dc.html`), not inferred.
const SIDEBAR_WIDTH_EXPANDED = '224px'
const SIDEBAR_WIDTH_COLLAPSED = '58px'

/** The sidebar's own tinted plate — the mock lifts it off the page background. */
const SIDEBAR_BG = 'var(--console-chrome-bg)'
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
      aria-current={active ? 'page' : undefined}
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
                  color: 'var(--console-context-text)',
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

          {/^⌘[1-9]$/.test(item.kbd) ? <span
            data-kbd="1"
            style={{
              flexShrink: 0,
              fontSize: 10,
              color: 'color-mix(in srgb, var(--aurora-text-muted) 65%, transparent)',
              fontVariantNumeric: 'tabular-nums',
            }}
          >
            {item.kbd}
          </span> : null}
        </>
      )}
    </Link>
  )
}

// ── Account card ──────────────────────────────────────────────────────────────

export function AccountMenu({ placement = 'sidebar' }: { placement?: 'sidebar' | 'topbar' }) {
  const collapsed = placement === 'topbar'
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
  const name = user ? email.split('@')[0] : 'Anonymous'
  const isDark = !mounted || resolvedTheme !== 'light'

  const signOut = async () => {
    setSigningOut(true)
    try {
      await logoutBrowserSession()
    } catch (error) {
      // Local state is already cleared; the server-side failure is reported
      // separately so the operator knows the server session may linger.
      toast.error(error instanceof LogoutRevocationError ? error.message : 'Sign-out could not be confirmed by the server.')
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
      style={{ padding: placement === 'topbar' ? 0 : '10px 10px 12px', marginLeft: placement === 'topbar' ? 2 : undefined, flexShrink: 0, minWidth: 0, position: 'relative' }}
    >
      {open ? (
        <div
          data-anim="menu"
          style={{
            position: 'fixed',
            top: placement === 'topbar' ? 48 : undefined,
            right: placement === 'topbar' ? 12 : undefined,
            bottom: placement === 'sidebar' ? 64 : undefined,
            left: placement === 'sidebar' ? 10 : undefined,
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
              <img src="/labby-avatar.png" alt="" style={{ width: '100%', height: '100%', borderRadius: 999, objectFit: 'cover' }} />
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
            <Link
              href="/settings"
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
                <Settings size={14} strokeWidth={1.7} />
              </span>
              <span style={{ flex: 1, whiteSpace: 'nowrap' }}>Settings</span>
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
        data-sidebar-toggle="1"
        type="button"
        onClick={() => setOpen((value) => !value)}
        aria-expanded={open}
        aria-label="Account menu"
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: placement === 'topbar' ? 7 : 9,
          width: placement === 'topbar' ? 60 : '100%',
          height: placement === 'topbar' ? 36 : undefined,
          padding: placement === 'topbar' ? '3px 8px 3px 3px' : '7px 8px',
          borderRadius: placement === 'topbar' ? 999 : 'var(--radius-1)',
          border: `1px solid ${
            hovered
              ? 'var(--aurora-border-strong)'
              : 'color-mix(in srgb, var(--aurora-border-default) 55%, var(--aurora-page-bg))'
          }`,
          background: hovered
            ? 'var(--aurora-hover-bg)'
            : 'var(--gw0-0_40)',
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
            width: placement === 'topbar' ? 28 : 30,
            height: placement === 'topbar' ? 28 : 30,
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
          <img src="/labby-avatar.png" alt="" style={{ width: '100%', height: '100%', borderRadius: 999, objectFit: 'cover' }} />
          <span
            style={{
              position: 'absolute',
              right: -1,
              bottom: -1,
              width: 8,
              height: 8,
              borderRadius: 999,
              background: user ? 'var(--aurora-success)' : 'var(--aurora-warn)',
              boxShadow: `0 0 4px ${user ? 'var(--aurora-success)' : 'var(--aurora-warn)'}, 0 0 0 2px ${SIDEBAR_BG}`,
            }}
          />
        </div>

        {placement === 'topbar' ? (
          <ChevronDown size={12} strokeWidth={1.8} style={{ flexShrink: 0, color: 'var(--aurora-text-muted)' }} />
        ) : null}

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
  const session = useBrowserSession()
  const authority = session.status === 'authenticated' ? session.authority : undefined
  const status = useConsoleStatus()
  const [stashSupported, setStashSupported] = React.useState(true)
  React.useEffect(() => {
    const controller = new AbortController()
    setStashSupported(true)
    if (authority?.capabilities.includes('scope.read')) {
      getStats(controller.signal).catch(error => {
        if (!controller.signal.aborted && error instanceof StashError && error.status === 404) setStashSupported(false)
      })
    }
    return () => controller.abort()
  }, [authority])

  const navSections = React.useMemo(
    () => capabilityAwareNavSections(authority?.capabilities ?? [], stashSupported),
    [authority?.capabilities, stashSupported],
  )
  const { collapsed, toggleCollapsed, mobileNavOpen, setMobileNavOpen } = useConsoleShell()
  const [isMobile, setIsMobile] = React.useState(false)

  const [pinned, setPinned] = React.useState<string[]>([])
  const [folded, setFolded] = React.useState<Record<string, boolean>>({})
  const [order, setOrder] = React.useState<Record<string, string[]>>({})
  const [sectionOrder, setSectionOrder] = React.useState<string[]>(SECTION_IDS)
  const sectionDragRef = React.useRef<string | null>(null)
  const orderedSections = [...navSections].sort((left, right) => sectionOrder.indexOf(left.id) - sectionOrder.indexOf(right.id))
  const depotRoute = ['/depot', '/create', '/library'].some(route => pathname === route || pathname.startsWith(`${route}/`))
  const workspaceRoute = ['/agents', '/tasks', '/dev-containers', '/projects', '/stash'].some(route => pathname === route || pathname.startsWith(`${route}/`))
  const teamRealm = workspaceRoute && (authority?.activeOwner.kind === 'team' || Boolean(authority?.activeTeamId))
  const realmColor = teamRealm ? 'var(--aurora-success)' : depotRoute ? 'var(--aurora-accent-strong)' : 'var(--aurora-accent-pink)'
  const realmBackground = teamRealm ? 'var(--aurora-success)' : depotRoute ? 'var(--aurora-accent-primary)' : 'var(--aurora-accent-pink)'
  const realmBorder = teamRealm
    ? 'color-mix(in srgb, var(--aurora-success) 32%, transparent)'
    : depotRoute
      ? 'color-mix(in srgb, var(--aurora-accent-primary) 30%, transparent)'
      : 'color-mix(in srgb, var(--aurora-accent-pink-deep) 38%, transparent)'
  const [toggleHovered, setToggleHovered] = React.useState(false)
  const [workspaceOpen, setWorkspaceOpen] = React.useState(false)
  const dragRef = React.useRef<{ section: string; id: string } | null>(null)
  const sidebarRef = React.useRef<HTMLElement>(null)

  React.useEffect(() => {
    setPinned(readJson<string[]>(PINNED_KEY, []))
    setFolded(readJson<Record<string, boolean>>(FOLDED_KEY, {}))
    setOrder(readJson<Record<string, string[]>>(ORDER_KEY, {}))
    const saved = readJson<unknown>(SECTION_ORDER_KEY, [])
    const valid = Array.isArray(saved) ? [...new Set(saved.filter((id): id is string => typeof id === 'string' && SECTION_IDS.includes(id)))] : []
    setSectionOrder([...valid, ...SECTION_IDS.filter(id => !valid.includes(id))])
  }, [])

  React.useEffect(() => {
    const media = window.matchMedia('(max-width: 900px)')
    const update = () => {
      setIsMobile(media.matches)
      if (!media.matches) setMobileNavOpen(false)
    }
    update()
    media.addEventListener('change', update)
    return () => media.removeEventListener('change', update)
  }, [setMobileNavOpen])

  React.useEffect(() => {
    setMobileNavOpen(false)
  }, [pathname, setMobileNavOpen])

  React.useEffect(() => {
    if (!isMobile || !mobileNavOpen) return
    const previousOverflow = document.body.style.overflow
    document.body.style.overflow = 'hidden'
    const focusableControls = () => Array.from(sidebarRef.current?.querySelectorAll<HTMLElement>(
      'a[href], button:not([disabled]), [tabindex]:not([tabindex="-1"])',
    ) ?? []).filter((element) => element.getClientRects().length > 0)
    window.requestAnimationFrame(() => focusableControls()[0]?.focus())
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault()
        setMobileNavOpen(false)
        return
      }
      if (event.key !== 'Tab') return
      const controls = focusableControls()
      if (controls.length === 0) return
      const first = controls[0]
      const last = controls[controls.length - 1]
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault()
        last.focus()
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault()
        first.focus()
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => {
      window.removeEventListener('keydown', onKeyDown)
      document.body.style.overflow = previousOverflow
      window.requestAnimationFrame(() => {
        document.querySelector<HTMLElement>('[data-mobile-menu]')?.focus()
      })
    }
  }, [isMobile, mobileNavOpen, setMobileNavOpen])

  const visuallyCollapsed = collapsed && !isMobile

  // ⌘/Ctrl + N jumps to the Nth nav item, matching the mock's accelerators.
  React.useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!event.metaKey && !event.ctrlKey) return
      if (event.altKey || event.shiftKey) return
      const index = Number.parseInt(event.key, 10)
      if (Number.isNaN(index) || index < 1) return
      const flat = navSections.flatMap((section) => section.items)
      const target = flat[index - 1]
      if (!target) return
      event.preventDefault()
      router.push(target.href)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [navSections, router])

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
    (section: (typeof navSections)[number]) => {
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

  const moveSection = React.useCallback((sourceId: string, targetId: string) => {
    if (sourceId === targetId || !SECTION_IDS.includes(sourceId) || !SECTION_IDS.includes(targetId)) return
    setSectionOrder(current => {
      const next = current.filter(id => id !== sourceId)
      next.splice(next.indexOf(targetId), 0, sourceId)
      writeJson(SECTION_ORDER_KEY, next)
      return next
    })
  }, [])

  const handleDrop = React.useCallback(
    (sectionId: string, targetId: string) => {
      const drag = dragRef.current
      dragRef.current = null
      if (!drag || drag.section !== sectionId || drag.id === targetId) return
      const section = navSections.find((entry) => entry.id === sectionId)
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
    [navSections, order],
  )

  return (
    <>
    {mobileNavOpen ? <button
      type="button"
      data-mobile-nav-backdrop="1"
      aria-label="Close navigation"
      onClick={() => setMobileNavOpen(false)}
      tabIndex={-1}
    /> : null}
    <aside
      ref={sidebarRef}
      id="console-navigation"
      data-console-sidebar="1"
      data-mobile-open={mobileNavOpen ? '1' : '0'}
      aria-hidden={isMobile && !mobileNavOpen ? true : undefined}
      aria-modal={isMobile && mobileNavOpen ? true : undefined}
      aria-label={isMobile && mobileNavOpen ? 'Navigation' : undefined}
      role={isMobile && mobileNavOpen ? 'dialog' : undefined}
      inert={isMobile && !mobileNavOpen ? true : undefined}
      style={{
        position: 'relative',
        width: visuallyCollapsed ? SIDEBAR_WIDTH_COLLAPSED : SIDEBAR_WIDTH_EXPANDED,
        flexShrink: 0,
        display: 'flex',
        flexDirection: 'column',
        background: SIDEBAR_BG,
        lineHeight: 'normal',
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
        data-sidebar-toggle="1"
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
        {visuallyCollapsed ? (
          <ChevronRight size={12} strokeWidth={1.7} />
        ) : (
          <ChevronLeft size={12} strokeWidth={1.7} />
        )}
      </button>

      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          height: '100%',
          minHeight: 0,
          overflow: 'visible',
        }}
      >
        {/* Brand */}
        <Link
          href="/depot"
          aria-label="Go to Discover"
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
            <LabbyIcon size={30} />
          </div>
          {visuallyCollapsed ? null : (
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
                data-realm-badge="1"
                style={{
                  display: 'inline-flex',
                  alignItems: 'center',
                  height: 15,
                  padding: '0 5px',
                  borderRadius: 4,
                  border: `1px solid ${realmBorder}`,
                  background: `color-mix(in srgb, ${realmBackground} 9%, transparent)`,
                  fontSize: 8,
                  fontWeight: 700,
                  letterSpacing: '0.12em',
                  color: realmColor,
                }}
              >
                LABBY
              </span>
            </div>
          )}
        </Link>

        {/* Nav */}
        <nav
          data-collapsed={visuallyCollapsed ? '1' : '0'}
          style={{
            flex: 1,
            display: 'flex',
            flexDirection: 'column',
            gap: 2,
            padding: '8px 8px 0',
            minWidth: 0,
            minHeight: 0,
            overflowY: 'auto',
            overflowX: 'hidden',
            scrollbarWidth: 'thin',
          }}
        >
          {orderedSections.map((section, sectionIndex) => {
            const isFolded = Boolean(folded[section.id])
            const items = orderedItems(section)

            return (
              <React.Fragment key={section.id}>
                {visuallyCollapsed && sectionIndex > 0 ? <div style={{ height: 1, flexShrink: 0, margin: '5px 6px', background: 'color-mix(in srgb, var(--aurora-border-default) 55%, var(--aurora-page-bg))' }} /> : null}
                {visuallyCollapsed ? null : (
                  <button
                    type="button"
                    data-nav-section={section.id}
                    draggable
                    title="Drag to reorder sections"
                    onDragStart={(event) => {
                      sectionDragRef.current = section.id
                      dragRef.current = null
                      if (event.dataTransfer) event.dataTransfer.effectAllowed = 'move'
                    }}
                    onDragOver={(event) => { if (sectionDragRef.current) event.preventDefault() }}
                    onDrop={(event) => {
                      event.preventDefault()
                      if (sectionDragRef.current) moveSection(sectionDragRef.current, section.id)
                      sectionDragRef.current = null
                    }}
                    onDragEnd={() => { sectionDragRef.current = null }}
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
                      color: 'var(--console-section-label)',
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

                {isFolded && !visuallyCollapsed ? null : (
                  <div
                    style={{
                      position: 'relative',
                      display: 'flex',
                      flexDirection: 'column',
                      gap: 2,
                      paddingLeft: visuallyCollapsed ? 0 : 8,
                    }}
                  >
                    {items.map((item) => (
                      <NavItem
                        key={item.id}
                        item={status.kind === 'ready' && (item.id === 'Overview' || item.id === 'Gateway') ? {
                          ...item,
                          contextLine: item.id === 'Overview'
                            ? `${status.snapshot.total} servers · ${status.snapshot.tools} tools`
                            : `${status.snapshot.total} servers · ${status.snapshot.total - status.snapshot.connected} disconnected`,
                        } : item}
                        sectionId={section.id}
                        active={isNavItemActive(item.href, pathname)}
                        collapsed={visuallyCollapsed}
                        pinned={pinned.includes(item.id)}
                        onTogglePin={togglePin}
                        onDragStart={(id) => {
                          sectionDragRef.current = null
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
        </nav>

        {/* Workspace switcher */}
        <div data-scopemenu="1" style={{ position: 'relative', flexShrink: 0, padding: '8px 8px 4px' }}>
          <button
            type="button"
            aria-label="Switch workspace"
            aria-expanded={workspaceOpen}
            onClick={() => {
              // The menu only has room on the expanded rail, so open the rail first.
              if (visuallyCollapsed) { toggleCollapsed(); setWorkspaceOpen(true); return }
              setWorkspaceOpen((value) => !value)
            }}
            style={{
              width: '100%', height: 38, borderRadius: 11,
              border: '1px solid color-mix(in srgb, var(--aurora-border-default) 55%, var(--aurora-page-bg))',
              background: 'var(--gw0-0_40)',
              color: 'var(--aurora-text-primary)', display: 'flex', alignItems: 'center', gap: 9,
              padding: visuallyCollapsed ? 0 : '0 10px 0 6px', justifyContent: visuallyCollapsed ? 'center' : 'flex-start',
              cursor: 'pointer', textAlign: 'left', fontFamily: 'inherit',
            }}
          >
            <span style={{ width: 24, height: 24, borderRadius: 8, display: 'grid', placeItems: 'center', flexShrink: 0, overflow: 'hidden', border: '1px solid color-mix(in srgb,var(--aurora-accent-primary) 34%,transparent)', color: 'var(--aurora-accent-strong)', fontSize: 9, fontWeight: 700 }}>
              {authority?.activeOwner.kind === 'team' ? authority.activeOwner.id.slice(0, 2).toUpperCase() : <img src="/labby-avatar.png" alt="" style={{ width: '100%', height: '100%', objectFit: 'cover' }}/>}
            </span>
            {visuallyCollapsed ? null : <><span style={{ minWidth: 0, flex: 1, display: 'flex', flexDirection: 'column', gap: 1 }}><small style={{ fontSize: 9, fontWeight: 700, letterSpacing: '.12em', color: 'var(--console-workspace-label)' }}>WORKSPACE</small><strong style={{ fontSize: 12, fontWeight: 650, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', paddingRight: 2 }}>{authority?.activeOwner.kind === 'personal' ? 'Personal' : authority?.projects.find(project => project.id === authority.activeOwner.id)?.name ?? authority?.activeOwner.id ?? 'No workspace'}</strong></span><ChevronsUpDown size={12} strokeWidth={1.8} color="var(--aurora-text-muted)"/></>}
          </button>
          {workspaceOpen && !visuallyCollapsed ? <div data-anim="menu" style={{ position: 'absolute', zIndex: 60, bottom: 'calc(100% + 4px)', left: 8, right: 8, minWidth: 210, padding: 5, borderRadius: 11, border: '1px solid var(--aurora-border-strong)', background: 'linear-gradient(180deg, var(--aurora-panel-strong-top), var(--aurora-panel-strong))', boxShadow: 'var(--aurora-shadow-strong), inset 0 1px 0 rgba(255,255,255,.05)' }}>
            <button type="button" data-menurow="1" disabled={!authority} aria-disabled={!authority} title={authority ? undefined : 'Workspace selection is unavailable until the server projects your authority.'} onClick={() => { if (switchWorkspace({})) { setWorkspaceOpen(false); router.push('/') } }} style={{ width: '100%', display: 'grid', gridTemplateColumns: '30px 1fr 16px', alignItems: 'center', gap: 7, padding: '7px 8px', border: 0, borderRadius: 8, background: authority?.activeOwner.kind === 'personal' ? 'var(--aurora-selected-bg)' : 'transparent', color: 'var(--aurora-text-primary)', textAlign: 'left', cursor: authority ? 'pointer' : 'not-allowed', opacity: authority ? 1 : 0.55 }}><span style={{ width: 28, height: 28, borderRadius: 999, display: 'grid', placeItems: 'center', overflow: 'hidden' }}><img src="/labby-avatar.png" alt="" style={{ width: '100%', height: '100%', borderRadius: 999, objectFit: 'cover' }}/></span><span><strong style={{ display: 'block', fontSize: 12.5 }}>Personal</strong><small style={{ display: 'block', maxWidth: 125, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', color: 'var(--aurora-text-muted)' }}>your private workspace</small></span>{authority?.activeOwner.kind === 'personal' ? <Check size={14} color="var(--aurora-accent-strong)"/> : null}</button>
            {authority && authority.teams.length === 0 ? <p className="px-2 py-2 text-[11px] text-aurora-text-muted">You are not currently a member of a Team.</p> : null}
            {authority?.teams.map((team) => <button key={team.id} type="button" data-menurow="1" onClick={() => { if (switchWorkspace({ teamId: team.id })) { setWorkspaceOpen(false); router.push('/') } }} style={{ width: '100%', display: 'grid', gridTemplateColumns: '30px 1fr 16px', alignItems: 'center', gap: 7, padding: '7px 8px', border: 0, borderRadius: 8, background: authority.activeTeamId === team.id ? 'var(--aurora-selected-bg)' : 'transparent', color: 'var(--aurora-text-primary)', textAlign: 'left', cursor: 'pointer' }}><span style={{ width: 27, height: 27, borderRadius: 7, display: 'grid', placeItems: 'center', background: 'color-mix(in srgb,var(--aurora-success) 12%,transparent)', border: '1px solid color-mix(in srgb,var(--aurora-success) 30%,transparent)', color: 'var(--aurora-success)', fontSize: 10 }}>{team.id.slice(0,2).toUpperCase()}</span><span><strong style={{ display: 'block', fontSize: 12.5 }}>{team.id}</strong><small style={{ color: 'var(--aurora-text-muted)' }}>{team.role}</small></span>{authority.activeTeamId === team.id ? <Check size={14} color="var(--aurora-accent-strong)"/> : null}</button>)}
            {authority?.projects.map((project) => <button key={project.id} type="button" data-menurow="1" onClick={() => { if (switchWorkspace({ projectId: project.id })) { setWorkspaceOpen(false); router.push('/') } }} style={{ width: '100%', display: 'grid', gridTemplateColumns: '30px 1fr 16px', alignItems: 'center', gap: 7, padding: '7px 8px', border: 0, borderRadius: 8, background: authority.activeProjectId === project.id ? 'var(--aurora-selected-bg)' : 'transparent', color: 'var(--aurora-text-primary)', textAlign: 'left', cursor: 'pointer' }}><span style={{ width: 27, height: 27, borderRadius: 7, display: 'grid', placeItems: 'center', background: 'color-mix(in srgb,var(--aurora-accent-primary) 12%,transparent)', border: '1px solid color-mix(in srgb,var(--aurora-accent-primary) 30%,transparent)', color: 'var(--aurora-accent-primary)', fontSize: 10 }}>PR</span><span style={{ minWidth: 0 }}><strong style={{ display: 'block', fontSize: 12.5, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{project.name ?? project.id}</strong><small style={{ display: 'block', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', color: 'var(--aurora-text-muted)' }}>{project.name ? `${project.id} · ` : ''}{project.role} · project</small></span>{authority.activeProjectId === project.id ? <Check size={14} color="var(--aurora-accent-strong)"/> : null}</button>)}
          </div> : null}
        </div>


      </div>
    </aside>
    </>
  )
}
