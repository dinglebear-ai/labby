import {
  Activity,
  Cable,
  CirclePlus,
  Clock3,
  Container,
  Bot,
  Inbox,
  Logs,
  Warehouse,
  GitBranch,
  LayoutDashboard,
  SearchCode,
  type LucideIcon,
} from 'lucide-react'

/**
 * Unified Labby + Depot information architecture. Depot owns Discover,
 * Create, and Library; Loadouts and Snippets are Library tabs rather than
 * parallel sidebar products. Workspace collects the agent-facing execution
 * surfaces, while operational telemetry remains under Observe.
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
        id: 'Logs',
        label: 'Logs',
        href: '/logs',
        icon: Logs,
        tooltipDetail: 'live control-plane and upstream events',
      },
    ],
  },
  {
    id: 'Depot',
    label: 'Depot',
    items: [
      {
        id: 'Discover',
        label: 'Discover',
        href: '/depot',
        icon: SearchCode,
        tooltipDetail: 'search the Depot Bazaar',
      },
      {
        id: 'Create',
        label: 'Create',
        href: '/create',
        icon: CirclePlus,
        tooltipDetail: 'author artifacts and bundles',
      },
      {
        id: 'Library',
        label: 'Library',
        href: '/library',
        icon: Warehouse,
        tooltipDetail: 'artifacts, loadouts and snippets',
      },
    ],
  },
  {
    id: 'Workspace',
    label: 'Workspace',
    items: [
      { id: 'Agents', label: 'Agents', href: '/agents', icon: Bot },
      { id: 'Tasks', label: 'Tasks', href: '/tasks', icon: Clock3 },
      { id: 'Stash', label: 'Stash', href: '/stash', icon: Inbox },
      { id: 'Dev Containers', label: 'Dev Containers', href: '/dev-containers', icon: Container },
    ],
  },
  {
    id: 'Observe', label: 'Observe', items: [
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
  if (href === '/library') return ['/library', '/loadouts', '/snippets'].some(
    (route) => pathname === route || pathname.startsWith(`${route}/`),
  )
  return pathname === href || pathname.startsWith(`${href}/`)
}
