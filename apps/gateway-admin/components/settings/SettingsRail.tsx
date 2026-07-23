'use client'

// Left nav rail for /settings/*. Static list of panels; URL-driven
// "active" state via usePathname.

import Link from 'next/link'
import { usePathname, useRouter } from 'next/navigation'

import { cn } from '@/lib/utils'

interface RailEntry {
  href: string
  label: string
}

const ENTRIES: RailEntry[] = [
  { href: '/settings/core/', label: 'Core' },
  { href: '/settings/surfaces/', label: 'Surfaces' },
  { href: '/settings/features/', label: 'Features' },
  { href: '/settings/deployment/', label: 'Deployment' },
  { href: '/settings/services/', label: 'Services' },
  { href: '/settings/doctor/', label: 'Doctor' },
  { href: '/settings/extract/', label: 'Extract' },
  { href: '/settings/advanced/', label: 'Advanced' },
]

export function SettingsRail(): React.ReactElement {
  const pathname = usePathname() ?? ''
  const router = useRouter()
  const activeEntry = ENTRIES.find((entry) => pathname.startsWith(entry.href)) ?? ENTRIES[0]
  const activeHref = activeEntry?.href ?? ENTRIES[0]?.href ?? ''
  return (
    <nav aria-label="Settings sections" className="px-4 py-3">
      <label htmlFor="settings-section" className="sr-only">
        Settings section
      </label>
      <select
        id="settings-section"
        value={activeHref}
        onChange={(event) => router.push(event.target.value)}
        className="h-9 w-full rounded-md border border-[#d4d4d4] bg-white px-3 text-sm font-medium text-[#1c1b1b] md:hidden"
      >
        {ENTRIES.map((entry) => (
          <option key={entry.href} value={entry.href}>
            {entry.label}
          </option>
        ))}
      </select>
      <div className="hidden items-center gap-1 overflow-x-auto md:flex">
        {ENTRIES.map((entry) => {
          const active = pathname.startsWith(entry.href)
          return (
            <Link
              key={entry.href}
              href={entry.href}
              aria-current={active ? 'page' : undefined}
              className={cn(
                'flex shrink-0 items-center rounded-md px-3 py-1.5 text-xs font-semibold no-underline transition-colors',
                active
                  ? 'bg-white text-[#1c1b1b] shadow-sm'
                  : 'text-[#737373] hover:bg-white/70 hover:text-[#1c1b1b]',
              )}
            >
              <span className="whitespace-nowrap">{entry.label}</span>
            </Link>
          )
        })}
      </div>
    </nav>
  )
}
