'use client'

// Section nav for /settings/*. Static list of panels; URL-driven "active"
// state via usePathname.
//
// The mock's Settings screen is a single 760px column with no sub-nav, so
// there is nothing to copy for a vertical rail. Rather than invent one, this
// renders the panels as the mock's own segmented-button control (28px tall,
// 8px radius, 11.5px/650, accent-tinted when active) in a horizontal strip
// above the column — which keeps the body sitting exactly where the mock puts
// it. Links, active state, and the mobile <select> fallback are unchanged.

import Link from 'next/link'
import { usePathname, useRouter } from 'next/navigation'
import {
  Activity,
  Cog,
  FileSearch,
  Layers,
  PlugZap,
  Server,
  Shield,
} from 'lucide-react'

import { settingsSegmentStyle, SETTINGS_CONTROL_STYLE } from './SettingsChrome'

interface RailEntry {
  href: string
  label: string
  icon: React.ComponentType<{ size?: number; className?: string }>
}

const ENTRIES: RailEntry[] = [
  { href: '/settings/core/', label: 'Core', icon: Cog },
  { href: '/settings/services/', label: 'Services', icon: Server },
  { href: '/settings/surfaces/', label: 'Surfaces', icon: PlugZap },
  { href: '/settings/features/', label: 'Features', icon: Layers },
  { href: '/settings/doctor/', label: 'Doctor', icon: Activity },
  { href: '/settings/extract/', label: 'Extract', icon: FileSearch },
  { href: '/settings/advanced/', label: 'Advanced', icon: Shield },
]

export function SettingsRail(): React.ReactElement {
  const pathname = usePathname() ?? ''
  const router = useRouter()
  const activeEntry = ENTRIES.find((entry) => pathname.startsWith(entry.href)) ?? ENTRIES[0]
  const activeHref = activeEntry?.href ?? ENTRIES[0]?.href ?? ''
  return (
    <nav aria-label="Settings sections">
      <label htmlFor="settings-section" className="sr-only">
        Settings section
      </label>
      <select
        id="settings-section"
        value={activeHref}
        onChange={(event) => router.push(event.target.value)}
        className="w-full md:hidden"
        style={{ ...SETTINGS_CONTROL_STYLE, width: '100%' }}
      >
        {ENTRIES.map((entry) => (
          <option key={entry.href} value={entry.href}>
            {entry.label}
          </option>
        ))}
      </select>
      <div
        className="hidden md:flex"
        style={{ gap: 4, flexWrap: 'wrap', alignItems: 'center' }}
      >
        {ENTRIES.map((entry) => {
          const active = pathname.startsWith(entry.href)
          const Icon = entry.icon
          return (
            <Link
              key={entry.href}
              href={entry.href}
              aria-current={active ? 'page' : undefined}
              style={settingsSegmentStyle(active)}
            >
              <Icon size={13} />
              <span>{entry.label}</span>
            </Link>
          )
        })}
      </div>
    </nav>
  )
}
