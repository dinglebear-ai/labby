import {
  Activity,
  BookOpen,
  Cable,
  FileCode2,
  LayoutDashboard,
  SearchCode,
  type LucideIcon,
} from 'lucide-react'

/**
 * Sidebar information architecture, mirroring the Gateway Console mock's
 * `defs` map. The mock declares exactly four sections:
 *
 *   Control Plane · Overview, Gateway, Loadouts
 *   Catalog       · Registry, Snippets
 *   Agents        · Sessions, Tasks
 *   Observe       · Files, Logs, Terminal
 *
 * with Settings pinned below the list. Section labels and ordering here match
 * that exactly. Two deliberate deviations, both forced by what this app
 * actually has:
 *
 *   - Loadouts, Registry, Sessions, Tasks, Files, Logs and Terminal are
 *     omitted. They have no route and no backing API, and shipping them would
 *     mean seven dead nav entries.
 *   - Skills (`/skills`) and Usage (`/usage`) are real routes with no mock
 *     counterpart, so they sit in the mock section they belong to rather than
 *     in a section the mock never had.
 *
 * `/docs` and `/design-system` are NOT top-level nav. The mock has no such
 * items; they live in the account popover instead.
 */

export type ConsoleNavItem = {
  /** Stable key — also the persistence identity for pinning and reordering. */
  id: string
  label: string
  href: string
  icon: LucideIcon
  /** Accelerator shown on hover and bound as ⌘/Ctrl+N. */
  kbd: string
  tooltip: string
  /** Sub-label rendered under the label while the item is the active route. */
  contextLine?: string
}

export type ConsoleNavSection = {
  id: string
  label: string
  items: ConsoleNavItem[]
}

export const consoleNavSections: ConsoleNavSection[] = [
  {
    id: 'Control Plane',
    label: 'Control Plane',
    items: [
      {
        id: 'Overview',
        label: 'Overview',
        href: '/',
        icon: LayoutDashboard,
        kbd: '⌘1',
        tooltip: 'Overview — ⌘1',
      },
      {
        id: 'Gateway',
        label: 'Gateway',
        href: '/gateways',
        icon: Cable,
        kbd: '⌘2',
        tooltip: 'Gateway — ⌘2 · upstream MCP servers',
      },
    ],
  },
  {
    id: 'Catalog',
    label: 'Catalog',
    items: [
      {
        id: 'Tools',
        label: 'Tools',
        href: '/tools',
        icon: SearchCode,
        kbd: '⌘3',
        tooltip: 'Tools — ⌘3 · live Code Mode catalog',
      },
      {
        id: 'Snippets',
        label: 'Snippets',
        href: '/snippets',
        icon: FileCode2,
        kbd: '⌘4',
        tooltip: 'Snippets — ⌘4 · Code Mode snippets',
      },
      {
        id: 'Skills',
        label: 'Skills',
        href: '/skills',
        icon: BookOpen,
        kbd: '⌘5',
        tooltip: 'Skills — ⌘5 · generated SKILL.md catalog',
      },
    ],
  },
  {
    id: 'Observe',
    label: 'Observe',
    items: [
      {
        id: 'Usage',
        label: 'Usage',
        href: '/usage',
        icon: Activity,
        kbd: '⌘6',
        tooltip: 'Usage — ⌘6 · tool call volume and latency',
      },
    ],
  },
]

export const consoleNavItems: ConsoleNavItem[] = consoleNavSections.flatMap(
  (section) => section.items,
)

/** Section id a given item belongs to — used by the pin affordance's label. */
export function sectionOf(itemId: string): string | undefined {
  return consoleNavSections.find((section) =>
    section.items.some((item) => item.id === itemId),
  )?.id
}

export function isNavItemActive(href: string, pathname: string): boolean {
  if (href === '/') return pathname === '/'
  return pathname === href || pathname.startsWith(`${href}/`)
}
