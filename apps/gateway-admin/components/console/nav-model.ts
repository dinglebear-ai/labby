import {
  Activity,
  BookOpen,
  Cable,
  FileCode2,
  Boxes,
  Warehouse,
  GitBranch,
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
 *   - Registry, Sessions, Tasks, Files, Logs and Terminal are omitted. They
 *     have no route and no backing API, and shipping them would mean dead nav
 *     entries. Loadouts is now a real gateway-backed route.
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

/** Raw item data, before the ⌘N accelerator is attached. */
type ConsoleNavItemSource = Omit<ConsoleNavItem, 'kbd' | 'tooltip'> & {
  /** Text appended after the label in the tooltip, e.g. "upstream MCP servers". */
  tooltipDetail?: string
}

type ConsoleNavSectionSource = {
  id: string
  label: string
  items: ConsoleNavItemSource[]
}

const CONSOLE_NAV_SOURCE: ConsoleNavSectionSource[] = [
  {
    id: 'Control Plane',
    label: 'Control Plane',
    items: [
      { id: 'Overview', label: 'Overview', href: '/', icon: LayoutDashboard },
      {
        id: 'Gateway',
        label: 'Gateway',
        href: '/gateways',
        icon: Cable,
        tooltipDetail: 'upstream MCP servers',
      },
      {
        id: 'Loadouts',
        label: 'Loadouts',
        href: '/loadouts',
        icon: Boxes,
        tooltipDetail: 'reusable gateway capability projections',
      },
    ],
  },
  {
    id: 'Catalog',
    label: 'Catalog',
    items: [
      {
        id: 'Depot',
        label: 'Depot',
        href: '/depot',
        icon: Warehouse,
        tooltipDetail: 'artifacts, ingestion and lifecycle',
      },
      {
        id: 'Tools',
        label: 'Tools',
        href: '/tools',
        icon: SearchCode,
        tooltipDetail: 'live Code Mode catalog',
      },
      {
        id: 'Snippets',
        label: 'Snippets',
        href: '/snippets',
        icon: FileCode2,
        tooltipDetail: 'Code Mode snippets',
      },
      {
        id: 'Skills',
        label: 'Skills',
        href: '/skills',
        icon: BookOpen,
        tooltipDetail: 'generated SKILL.md catalog',
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
        tooltipDetail: 'tool call volume and latency',
      },
      {
        id: 'Traces',
        label: 'Traces',
        href: '/traces',
        icon: GitBranch,
        tooltipDetail: 'correlated request flows',
      },
    ],
  },
]

// The ⌘/Ctrl+N handler in console-sidebar.tsx binds N to the Nth item of
// `consoleNavSections.flatMap(section => section.items)`, in section order.
// The accelerator shown here is derived from that same flattened position
// instead of being typed per item, so the two can never drift apart again —
// they previously did: Loadouts was inserted into Control Plane without
// renumbering anything after it, leaving Tools and Loadouts both labelled
// ⌘3, and Usage/Traces both labelled ⌘6, none of which matched what the
// handler actually bound.
let flatIndex = 0
export const consoleNavSections: ConsoleNavSection[] = CONSOLE_NAV_SOURCE.map((section) => ({
  id: section.id,
  label: section.label,
  items: section.items.map((item) => {
    flatIndex += 1
    const kbd = `⌘${flatIndex}`
    return {
      id: item.id,
      label: item.label,
      href: item.href,
      icon: item.icon,
      contextLine: item.contextLine,
      kbd,
      tooltip: item.tooltipDetail
        ? `${item.label} — ${kbd} · ${item.tooltipDetail}`
        : `${item.label} — ${kbd}`,
    }
  }),
}))

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
