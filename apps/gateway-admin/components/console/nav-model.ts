import {
  Activity,
  Archive,
  Bot,
  BookOpen,
  Box,
  Cable,
  CalendarClock,
  FileCode2,
  Boxes,
  GitBranch,
  FolderOpen,
  FolderKanban,
  LayoutDashboard,
  PackageSearch,
  Plus,
  SearchCode,
  ScrollText,
  Shield,
  type LucideIcon,
} from 'lucide-react'

/** Sidebar information architecture for the scoped Depot console shell. */

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
  /** True when the context line is an illustrative fixture rather than live data. */
  contextIsMock?: boolean
}

export type ConsoleNavSection = {
  id: string
  label: string
  items: ConsoleNavItem[]
}

export type ConsoleWorkspaceScope = 'personal' | 'team'

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
      { id: 'Overview', label: 'Overview', href: '/', icon: LayoutDashboard, contextLine: 'fleet activity · 24h', contextIsMock: true },
      {
        id: 'Gateway',
        label: 'Gateway',
        href: '/gateways',
        icon: Cable,
        tooltipDetail: 'upstream MCP servers',
        contextLine: '16 servers · 3 need attention',
        contextIsMock: true,
      },
      {
        id: 'Loadouts',
        label: 'Loadouts',
        href: '/loadouts',
        icon: Boxes,
        tooltipDetail: 'reusable gateway capability projections',
      },
      {
        id: 'Instance',
        label: 'Labby',
        href: '/instance',
        icon: Cable,
        tooltipDetail: 'mock hosted instance',
        contextLine: 'hosted · eu-west',
        contextIsMock: true,
      },
    ],
  },
  {
    id: 'Depot',
    label: 'Depot',
    items: [
      { id: 'Discovery', label: 'Discovery', href: '/discovery', icon: PackageSearch, tooltipDetail: 'mock artifact bazaar', contextLine: '26 artifacts · 9 sources', contextIsMock: true },
      { id: 'Create', label: 'Create', href: '/create', icon: Plus, tooltipDetail: 'mock artifact authoring', contextLine: 'artifacts + bundles', contextIsMock: true },
      { id: 'Library', label: 'Library', href: '/library', icon: FolderOpen, tooltipDetail: 'mock personal artifact library', contextLine: '40 saved artifacts', contextIsMock: true },
    ],
  },
  {
    id: 'Workspace',
    label: 'Workspace',
    items: [
      { id: 'Agents', label: 'Agents', href: '/agents', icon: Bot, tooltipDetail: 'mock agent launcher', contextLine: '2 running · 1 failed', contextIsMock: true },
      { id: 'WorkspaceTasks', label: 'Tasks', href: '/tasks', icon: CalendarClock, tooltipDetail: 'mock recurring agent runs', contextLine: '4 scheduled · 3 armed', contextIsMock: true },
      { id: 'Stash', label: 'Stash', href: '/stash', icon: Archive, tooltipDetail: 'mock agent working files', contextLine: '7 files · 194 MB', contextIsMock: true },
      { id: 'Containers', label: 'Dev Containers', href: '/containers', icon: Box, tooltipDetail: 'mock Incus system containers', contextLine: '3 images · 1 building', contextIsMock: true },
    ],
  },
  {
    id: 'Team',
    label: 'Team · Mock',
    items: [
      { id: 'TeamOverview', label: 'Overview · Mock', href: '/team', icon: Shield, tooltipDetail: 'mock tootie.tv workspace', contextLine: '9 members · 3 projects', contextIsMock: true },
      { id: 'TeamLibrary', label: 'Library · Mock', href: '/team/library', icon: BookOpen, tooltipDetail: 'mock shared artifact library', contextLine: '40 shared artifacts', contextIsMock: true },
      { id: 'TeamProjects', label: 'Projects · Mock', href: '/team/projects', icon: FolderKanban, tooltipDetail: 'mock project bindings', contextLine: '3 active projects', contextIsMock: true },
      { id: 'TeamActivity', label: 'Activity · Mock', href: '/team/activity', icon: Activity, tooltipDetail: 'mock team feed', contextLine: '12 new events', contextIsMock: true },
      { id: 'TeamStash', label: 'Stash · Mock', href: '/team/stash', icon: Archive, tooltipDetail: 'mock shared working files', contextLine: '7 files · 194 MB', contextIsMock: true },
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
        id: 'Logs',
        label: 'Logs',
        href: '/logs',
        icon: ScrollText,
        tooltipDetail: 'mock unified event stream',
        contextLine: 'all sources · following',
        contextIsMock: true,
      },
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
    const kbd = flatIndex <= 9 ? `⌘${flatIndex}` : ''
    return {
      id: item.id,
      label: item.label,
      href: item.href,
      icon: item.icon,
      contextLine: item.contextLine,
      contextIsMock: item.contextIsMock,
      kbd,
      tooltip: [item.label, kbd, item.tooltipDetail].filter(Boolean).join(' · '),
    }
  }),
}))

export const consoleNavItems: ConsoleNavItem[] = consoleNavSections.flatMap(
  (section) => section.items,
)

/** Team destinations belong to the mock team workspace, not the personal rail. */
export function consoleNavSectionsForScope(
  scope: ConsoleWorkspaceScope,
): ConsoleNavSection[] {
  const sectionIds =
    scope === 'team'
      ? ['Control Plane', 'Depot', 'Workspace', 'Team']
      : ['Control Plane', 'Depot', 'Workspace']
  const itemIds: Record<string, string[]> = {
    'Control Plane': ['Overview', 'Gateway', 'Instance', 'Logs'],
    Depot: ['Discovery', 'Create', 'Library'],
    Workspace: ['Agents', 'WorkspaceTasks', 'Stash', 'Containers'],
    Team: ['TeamOverview', 'TeamProjects', 'TeamActivity'],
  }
  const accelerators: Record<string, string> = {
    Discovery: '⌘1',
    Create: '⌘2',
    Library: '⌘3',
    Overview: '⌘4',
    Gateway: '⌘5',
    Logs: '⌘6',
  }

  return sectionIds.flatMap((sectionId) => {
    const section = consoleNavSections.find((entry) => entry.id === sectionId)
    if (!section) return []
    return [
      {
        ...section,
        label: sectionId,
        items: itemIds[sectionId].flatMap((itemId) => {
          const item = consoleNavItems.find((entry) => entry.id === itemId)
          if (!item) return []
          const href =
            scope === 'team' && item.id === 'Library'
              ? '/team/library'
              : scope === 'team' && item.id === 'Stash'
                ? '/team/stash'
                : item.href
          const label =
            item.id === 'TeamOverview'
              ? 'Overview'
              : item.id === 'TeamProjects'
                ? 'Projects'
                : item.id === 'TeamActivity'
                  ? 'Activity'
                  : item.label
          const kbd = accelerators[item.id] ?? ''
          return [
            {
              ...item,
              label,
              href,
              kbd,
              tooltip: [label, kbd, item.tooltip.split(' · ').at(-1)]
                .filter(Boolean)
                .join(' · '),
            },
          ]
        }),
      },
    ]
  })
}

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
