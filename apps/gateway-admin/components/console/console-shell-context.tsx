'use client'

import * as React from 'react'

/**
 * Shell coordination for the Gateway Console chrome.
 *
 * The mock renders exactly one topbar for the whole console, with each screen
 * contributing only its breadcrumb leaf and its action cluster. Pages keep
 * using `<AppHeader />`, which portals those two fragments into the slots the
 * shell exposes here — no per-page header markup, no state round-trips.
 */

const SIDEBAR_STORAGE_KEY = 'labby-sidebar-collapsed-v2'

type ConsoleShellContextValue = {
  collapsed: boolean
  setSidebarCollapsed: (collapsed: boolean) => void
  toggleCollapsed: () => void
  mobileNavOpen: boolean
  setMobileNavOpen: (open: boolean) => void
  toggleMobileNav: () => void
  crumbSlot: HTMLElement | null
  setCrumbSlot: (node: HTMLElement | null) => void
  actionSlot: HTMLElement | null
  setActionSlot: (node: HTMLElement | null) => void
}

const ConsoleShellContext = React.createContext<ConsoleShellContextValue | null>(null)

export function useConsoleShell() {
  const ctx = React.useContext(ConsoleShellContext)
  if (!ctx) throw new Error('useConsoleShell must be used inside <ConsoleShellProvider>')
  return ctx
}

/** Non-throwing variant — lets `AppHeader` render harmlessly outside the shell (tests, storybook). */
export function useOptionalConsoleShell() {
  return React.useContext(ConsoleShellContext)
}

export function ConsoleShellProvider({ children }: { children: React.ReactNode }) {
  // The supplied Gateway Console reference opens on the compact 58px icon rail.
  // A user's explicit persisted sidebar choice still wins after mount.
  const [collapsed, setCollapsed] = React.useState(true)
  const [mobileNavOpen, setMobileNavOpen] = React.useState(false)
  const [crumbSlot, setCrumbSlot] = React.useState<HTMLElement | null>(null)
  const [actionSlot, setActionSlot] = React.useState<HTMLElement | null>(null)

  // Read persisted state after mount so SSR and first client paint agree.
  React.useEffect(() => {
    try {
      const saved = window.localStorage.getItem(SIDEBAR_STORAGE_KEY)
      setCollapsed(saved === null ? true : saved === '1')
    } catch {
      /* storage unavailable — keep the mock's compact default */
    }
  }, [])

  const toggleCollapsed = React.useCallback(() => {
    setCollapsed((current) => {
      const next = !current
      try {
        window.localStorage.setItem(SIDEBAR_STORAGE_KEY, next ? '1' : '0')
      } catch {
        /* ignore */
      }
      return next
    })
  }, [])

  const toggleMobileNav = React.useCallback(() => {
    setMobileNavOpen((current) => !current)
  }, [])

  const value = React.useMemo<ConsoleShellContextValue>(
    () => ({
      collapsed,
      setSidebarCollapsed: setCollapsed,
      toggleCollapsed,
      mobileNavOpen,
      setMobileNavOpen,
      toggleMobileNav,
      crumbSlot,
      setCrumbSlot,
      actionSlot,
      setActionSlot,
    }),
    [collapsed, toggleCollapsed, mobileNavOpen, toggleMobileNav, crumbSlot, actionSlot],
  )

  return <ConsoleShellContext.Provider value={value}>{children}</ConsoleShellContext.Provider>
}
