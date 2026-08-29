import type { LucideIcon } from 'lucide-react'
import {
  Bot,
  CalendarClock,
  CheckCircle2,
  CirclePause,
  Clock3,
  Container,
  GitBranch,
  Play,
  ScrollText,
  TerminalSquare,
} from 'lucide-react'

import { AppHeader } from '@/components/app-header'
import { MockSurfaceBadge } from '@/components/console/mock-surface-badge'
import { cn } from '@/lib/utils'

type MockPageKind = 'sessions' | 'tasks' | 'logs'

const PAGE_META: Record<MockPageKind, { eyebrow: string; title: string; description: string; icon: LucideIcon }> = {
  sessions: {
    eyebrow: 'Agents · Workspaces',
    title: 'Sessions',
    description: 'Agent workspaces paired with a loadout, container, and repository.',
    icon: Bot,
  },
  tasks: {
    eyebrow: 'Team · Schedules',
    title: 'Tasks',
    description: 'Recurring agent runs with their own loadout, container, and repository.',
    icon: CalendarClock,
  },
  logs: {
    eyebrow: 'Observability',
    title: 'Logs',
    description: 'A unified stream of gateway, runner, and operator events.',
    icon: ScrollText,
  },
}

const CARD =
  'overflow-hidden rounded-aurora-2 border border-[color-mix(in_srgb,var(--aurora-border-default)_45%,var(--aurora-page-bg))] bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] shadow-[var(--aurora-shadow-medium),inset_0_1px_0_rgba(255,255,255,0.04)]'

function MockNotice() {
  return (
    <div className="flex items-start gap-3 rounded-aurora-1 border border-aurora-warn/30 bg-[color-mix(in_srgb,var(--aurora-warn)_8%,var(--aurora-panel-strong))] px-4 py-3 text-[12px] leading-[1.55] text-aurora-text-muted">
      <MockSurfaceBadge className="mt-0.5 shrink-0" />
      <p>
        This screen mirrors the approved console mock. Its rows and metrics are illustrative;
        Labby does not currently expose a backing runtime contract for this surface.
      </p>
    </div>
  )
}

function Hero({ kind }: { kind: MockPageKind }) {
  const meta = PAGE_META[kind]
  const Icon = meta.icon

  return (
    <section
      data-hero="1"
      className="overflow-hidden rounded-aurora-3 border border-[color-mix(in_srgb,var(--aurora-border-default)_45%,var(--aurora-page-bg))] bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] shadow-[var(--aurora-shadow-strong),inset_0_1px_0_rgba(255,255,255,0.05)]"
    >
      <div className="flex flex-wrap items-end justify-between gap-4 px-6 pt-[22px] pb-[18px]">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2.5">
            <span className="text-[10.5px] font-bold uppercase tracking-[0.16em] text-aurora-text-muted">
              {meta.eyebrow}
            </span>
            <MockSurfaceBadge />
          </div>
          <div className="mt-2 flex items-center gap-3">
            <Icon className="size-7 text-aurora-accent-strong" strokeWidth={1.6} aria-hidden="true" />
            <h1 className="font-display text-[30px] leading-[1.04] font-extrabold text-aurora-text-primary">
              {meta.title}
            </h1>
          </div>
          <p className="mt-[7px] max-w-[560px] text-[12.5px] leading-[1.55] text-aurora-text-muted">
            {meta.description}
          </p>
        </div>
        <button
          type="button"
          disabled
          title="Unavailable — mock surface"
          className="inline-flex h-9 cursor-not-allowed items-center gap-2 rounded-[10px] border border-aurora-border-default bg-aurora-control-surface px-4 text-[13px] font-semibold text-aurora-text-muted opacity-65"
        >
          <Play className="size-3.5" aria-hidden="true" />
          {kind === 'tasks' ? 'New Task' : kind === 'sessions' ? 'New Session' : 'Live tail'}
        </button>
      </div>
    </section>
  )
}

const sessions = [
  ['gateway-release', 'Running', 'project-a-loadout', 'platform-base', 'dinglebear-ai/labby', '18m'],
  ['axon-ingest-check', 'Waiting', 'research', 'rust-heavy', 'dinglebear-ai/axon', '42m'],
  ['docs-sweep', 'Complete', 'platform-core', 'edge-minimal', 'dinglebear-ai/labby', '2h'],
]

function SessionsPanel() {
  return (
    <section aria-label="Mock sessions" className={CARD} data-mock-region="sessions">
      <PanelHeader label="Active sessions" count="3 illustrative rows" />
      <div className="hidden grid-cols-[minmax(180px,1.3fr)_100px_minmax(150px,1fr)_140px_minmax(180px,1fr)_60px] gap-3 border-b border-aurora-border-strong bg-[var(--gw0-0_48)] px-4 py-2.5 text-[10px] font-bold uppercase tracking-[0.14em] text-aurora-text-muted md:grid">
        <span>Session</span><span>Status</span><span>Loadout</span><span>Container</span><span>Repository</span><span>Age</span>
      </div>
      {sessions.map(([name, status, loadout, container, repo, age]) => (
        <div key={name} className="grid gap-2 border-t border-aurora-border-default/55 px-4 py-3 first:border-t-0 md:grid-cols-[minmax(180px,1.3fr)_100px_minmax(150px,1fr)_140px_minmax(180px,1fr)_60px] md:items-center md:gap-3">
          <span className="font-display text-[13px] font-bold text-aurora-text-primary">{name}</span>
          <Status label={status} />
          <span className="text-[11.5px] text-aurora-text-muted">{loadout}</span>
          <span className="inline-flex items-center gap-1.5 text-[11.5px] text-aurora-text-muted"><Container className="size-3" />{container}</span>
          <span className="inline-flex min-w-0 items-center gap-1.5 truncate text-[11.5px] text-aurora-text-muted"><GitBranch className="size-3 shrink-0" />{repo}</span>
          <span className="text-[11px] tabular-nums text-aurora-text-muted">{age}</span>
        </div>
      ))}
    </section>
  )
}

const tasks = [
  ['Nightly gateway audit', 'Every day · 02:00', 'gateway-release', 'Succeeded', '6h ago'],
  ['Dependency drift scan', 'Mon–Fri · 08:30', 'platform-core', 'Running', '12m'],
  ['Axon benchmark digest', 'Friday · 17:00', 'research', 'Paused', '4d ago'],
]

function TasksPanel() {
  return (
    <section aria-label="Mock scheduled tasks" className={CARD} data-mock-region="tasks">
      <PanelHeader label="Scheduled" count="3 illustrative rows" />
      {tasks.map(([name, schedule, loadout, state, lastRun]) => (
        <div key={name} className="grid gap-3 border-t border-aurora-border-default/55 px-4 py-3 first:border-t-0 sm:grid-cols-[minmax(180px,1.4fr)_minmax(150px,1fr)_minmax(130px,1fr)_90px_90px] sm:items-center">
          <span className="font-display text-[13px] font-bold text-aurora-text-primary">{name}</span>
          <span className="inline-flex items-center gap-1.5 text-[11.5px] text-aurora-text-muted"><Clock3 className="size-3" />{schedule}</span>
          <span className="text-[11.5px] text-aurora-text-muted">{loadout}</span>
          <Status label={state} />
          <span className="text-[11px] tabular-nums text-aurora-text-muted">{lastRun}</span>
        </div>
      ))}
    </section>
  )
}

const logs = [
  ['14:32:08.418', 'INFO', 'gateway', 'Reconciled 16 upstream servers'],
  ['14:32:07.991', 'DEBUG', 'runner', 'Code Mode worker returned to warm pool'],
  ['14:31:58.204', 'WARN', 'oauth', 'Token refresh scheduled for github-mcp'],
  ['14:31:42.106', 'INFO', 'mcp', 'tools/list completed · 127 tools · 38 ms'],
  ['14:31:39.774', 'ERROR', 'upstream', 'mcp.sh probe timed out after 10 s'],
]

function LogsPanel() {
  return (
    <section aria-label="Mock log stream" className={cn(CARD, 'bg-[var(--gw4-0_62)]')} data-mock-region="logs" data-dark-island="1">
      <PanelHeader label="Logs" count="Illustrative stream" />
      <div className="aurora-scrollbar overflow-x-auto p-3 font-mono text-[11.5px] leading-7">
        {logs.map(([time, level, source, message]) => (
          <div key={`${time}-${source}`} className="grid min-w-[720px] grid-cols-[100px_64px_100px_1fr] gap-3 rounded-md px-2 hover:bg-aurora-hover-bg">
            <span className="text-aurora-text-muted">{time}</span>
            <span className={level === 'ERROR' ? 'text-aurora-error' : level === 'WARN' ? 'text-aurora-warn' : level === 'INFO' ? 'text-aurora-success' : 'text-aurora-accent-strong'}>{level}</span>
            <span className="text-aurora-accent-pink">{source}</span>
            <span className="text-aurora-text-primary">{message}</span>
          </div>
        ))}
      </div>
      <div className="flex items-center gap-2 border-t border-aurora-border-default/70 bg-[var(--gw0-0_30)] px-4 py-2 text-[10.5px] text-aurora-text-muted">
        <TerminalSquare className="size-3" aria-hidden="true" />
        Static preview · live streaming is not connected
      </div>
    </section>
  )
}

function PanelHeader({ label, count }: { label: string; count: string }) {
  return (
    <div className="flex items-center gap-2 border-b border-aurora-border-default/70 bg-[var(--gw0-0_38)] px-4 py-2.5">
      <span className="text-[10px] font-bold uppercase tracking-[0.14em] text-aurora-text-muted">{label}</span>
      <span className="ml-auto text-[10.5px] tabular-nums text-aurora-text-muted">{count}</span>
      <MockSurfaceBadge />
    </div>
  )
}

function Status({ label }: { label: string }) {
  const tone = label === 'Running' || label === 'Succeeded'
    ? 'text-aurora-success border-aurora-success/30 bg-[color-mix(in_srgb,var(--aurora-success)_9%,transparent)]'
    : label === 'Paused' || label === 'Waiting'
      ? 'text-aurora-warn border-aurora-warn/30 bg-[color-mix(in_srgb,var(--aurora-warn)_9%,transparent)]'
      : 'text-aurora-text-muted border-aurora-border-default bg-aurora-control-surface'
  const Icon = label === 'Paused' ? CirclePause : CheckCircle2
  return <span className={cn('inline-flex w-fit items-center gap-1.5 rounded-md border px-2 py-1 text-[10px] font-semibold', tone)}><Icon className="size-3" />{label}</span>
}

export function MockConsolePage({ kind }: { kind: MockPageKind }) {
  const meta = PAGE_META[kind]
  return (
    <>
      <AppHeader breadcrumbs={[{ label: meta.title }]} />
      <section data-screen-label={meta.title} className="flex flex-col gap-[14px]">
        <Hero kind={kind} />
        <MockNotice />
        {kind === 'sessions' ? <SessionsPanel /> : kind === 'tasks' ? <TasksPanel /> : <LogsPanel />}
      </section>
    </>
  )
}
