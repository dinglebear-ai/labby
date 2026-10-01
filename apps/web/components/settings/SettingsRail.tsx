'use client'

import Link from 'next/link'
import { usePathname, useRouter } from 'next/navigation'
import {
  Activity,
  Bot,
  Bell,
  Cog,
  FileSearch,
  KeyRound,
  Layers,
  ListChecks,
  PlugZap,
  Server,
  Shield,
  Warehouse,
} from 'lucide-react'
import { useBrowserSession, type BrowserSessionState } from '@/lib/auth/session'

import { settingsSegmentStyle, SETTINGS_CONTROL_STYLE } from './SettingsChrome'

interface RailEntry {
  href: string
  label: string
  icon: React.ComponentType<{ size?: number; className?: string }>
}

const ENTRIES: RailEntry[] = [
  { href: '/settings/', label: 'Overview', icon: ListChecks },
  { href: '/settings/core/', label: 'Core', icon: Cog },
  { href: '/settings/agents/', label: 'Agent provider', icon: Bot },
  { href: '/settings/services/', label: 'Services', icon: Server },
  { href: '/settings/surfaces/', label: 'Surfaces', icon: PlugZap },
  { href: '/settings/features/', label: 'Features', icon: Layers },
  { href: '/settings/doctor/', label: 'Doctor', icon: Activity },
  { href: '/settings/extract/', label: 'Extract', icon: FileSearch },
  { href: '/settings/advanced/', label: 'Advanced', icon: Shield },
]

/**
 * The panels a session may open. Depot needs platform administration; the
 * Authentication panel (administrator list and sign-in allowlist) is
 * accepted by the server only from a configured admin's browser session, so
 * `isAdmin` alone does not offer it.
 */
export function settingsRailEntries(session: BrowserSessionState): RailEntry[] {
  if (session.status !== 'authenticated' || !session.isAdmin) return ENTRIES
  return [
    ...ENTRIES,
    { href: '/settings/notifications/', label: 'Notifications', icon: Bell },
    ...(session.isConfiguredAdmin
      ? [{ href: '/settings/authentication/', label: 'Authentication', icon: KeyRound }]
      : []),
    { href: '/settings/depot/', label: 'Depot', icon: Warehouse },
  ]
}

export function activeSettingsHref(pathname: string, entries: RailEntry[]): string {
  const normalizedPath = pathname.endsWith('/') ? pathname : `${pathname}/`
  return entries.find((entry) => entry.href !== '/settings/' && normalizedPath.startsWith(entry.href))?.href
    ?? entries.find((entry) => entry.href === normalizedPath)?.href
    ?? entries[0]?.href
    ?? ''
}

export function SettingsRail(): React.ReactElement {
  const pathname = usePathname() ?? ''
  const router = useRouter()
  const session = useBrowserSession()
  const entries = settingsRailEntries(session)
  const activeHref = activeSettingsHref(pathname, entries)

  return (
    <nav aria-label="Settings sections" className="self-start lg:sticky lg:top-0">
      <label htmlFor="settings-section" className="sr-only">
        Settings section
      </label>
      <select
        id="settings-section"
        value={activeHref}
        onChange={(event) => router.push(event.target.value)}
        name="settings-section"
        className="w-full lg:hidden"
        style={{ ...SETTINGS_CONTROL_STYLE, width: '100%' }}
      >
        {entries.map((entry) => (
          <option key={entry.href} value={entry.href}>
            {entry.label}
          </option>
        ))}
      </select>
      <div className="hidden space-y-6 lg:block">
        {['Workspace', 'Connections', 'System'].map((group) => {
          const grouped = entries.filter((entry) => {
            const section = entry.href.split('/')[2]
            return (['', 'core', 'agents', 'services'].includes(section) ? 'Workspace' : ['surfaces', 'authentication', 'depot'].includes(section) ? 'Connections' : 'System') === group
          })
          return <div key={group}>
            <p className="mb-2 px-3 text-xs font-medium text-aurora-text-muted">{group}</p>
            <div className="space-y-1">{grouped.map((entry) => {
              const active = entry.href === activeHref
              const Icon = entry.icon
              return <Link key={entry.href} href={entry.href} aria-current={active ? 'page' : undefined}
                className="w-full focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-aurora-accent-primary"
                style={{ ...settingsSegmentStyle(active), justifyContent: 'flex-start', height: 40, gap: 10, background: active ? 'color-mix(in srgb, var(--aurora-accent-primary) 12%, transparent)' : 'transparent', borderColor: active ? 'var(--aurora-border-strong)' : 'transparent' }}>
                <Icon size={16} /><span>{entry.label}</span>
              </Link>
            })}</div>
          </div>
        })}
      </div>
    </nav>
  )
}
