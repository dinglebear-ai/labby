import { Bot, CalendarClock, Clock3, Play, Square } from 'lucide-react'

import { AppHeader } from '@/components/app-header'
import { MockSurfaceBadge } from '@/components/console/mock-surface-badge'

const sessions = [
  ['running', 'Refactor gateway reconcile loop', 'tootie-tv/labby · jmagar', 'project-a-loadout', 'platform-base', '12m'],
  ['running', 'Port ingest pipeline to streaming', 'tootie-tv/axon · carla ruiz', 'platform-core', 'rust-heavy', '4m'],
  ['completed', 'Audit loadout scopes for write tools', 'tootie-tv/depot · ben okafor', 'project-a-loadout', 'platform-base', '2m 18s'],
  ['completed', 'Summarize gateway error forensics', 'tootie-tv/labby · erin walsh', 'oncall-loadout', 'platform-base', '9m 55s'],
  ['completed', 'Generate SKILL.md for unRAID server', 'tootie-tv/depot · gia moreno', 'platform-core', 'edge-minimal', '58s'],
  ['failed', 'Draft plugin.json for Apprise', 'tootie-tv/depot · dev patel', 'project-b-loadout', 'rust-heavy', '41s'],
] as const

const tasks = [
  ['Loadout Scope Audit', 'Flags write-capable tools added since the last run', 'Daily · 02:00', '#project-a', 'in 6h', true],
  ['Error Forensics Digest', 'Clusters gateway errors and posts a digest to Activity', 'Mon, Thu · 07:00', '#platform', 'in 2d', true],
  ['Upstream Drift Sweep', 'Merges clean upstream updates, opens diffs for the rest', 'Weekly · Sun 03:00', '#shared', 'in 4d', true],
  ['Container Rebuild', 'Rebuilds every image against the pinned toolchain set', 'Weekly · Sat 01:00', '#platform', 'in 3d', true],
  ['Dependency Bump PR', 'Opens a PR per repo with the safe semver bumps', 'Daily · 05:00', '#project-b', 'paused', false],
] as const

const card = 'overflow-hidden rounded-aurora-2 border border-aurora-border-default/55 bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] shadow-[var(--aurora-shadow-medium),inset_0_1px_0_rgba(255,255,255,0.04)]'
const control = 'inline-flex h-8 items-center gap-1.5 rounded-[9px] border border-aurora-border-default bg-aurora-control-surface px-3 text-[11px] font-semibold text-aurora-text-muted disabled:cursor-not-allowed disabled:opacity-60'

function MockNotice({ noun }: { noun: string }) {
  return <div className="flex flex-col items-start gap-2.5 rounded-aurora-1 border border-aurora-warn/30 bg-[color-mix(in_srgb,var(--aurora-warn)_8%,var(--aurora-panel-strong))] px-4 py-3 text-[12px] leading-[1.55] text-aurora-text-muted sm:flex-row sm:gap-3"><MockSurfaceBadge /><p>This {noun} surface reproduces the approved mock. Every row, metric, schedule, and action is illustrative; no controls call a Labby service.</p></div>
}

export function MockAgentsPage() {
  return <>
    <AppHeader breadcrumbs={[{ label: 'Agents' }]} />
    <section data-screen-label="Agents" data-mock-region="agents" aria-label="Agents mock data" className="flex flex-col gap-[14px]">
      <section data-hero="1" className={card}>
        <div className="flex flex-wrap items-end justify-between gap-4 px-6 pt-[22px] pb-4"><div><div className="flex items-center gap-2.5"><span className="text-[10.5px] font-bold uppercase tracking-[.16em] text-aurora-text-muted">Team · Agents</span><span className="inline-flex items-center gap-1.5 text-[10.5px] font-semibold text-aurora-success"><span className="size-1.5 rounded-full bg-aurora-success" />2 running</span><MockSurfaceBadge /></div><div className="mt-2 flex items-center gap-3"><Bot className="size-7 text-aurora-accent-strong" /><h1 className="font-display text-[30px] font-extrabold text-aurora-text-primary">Agents</h1></div></div><button disabled title="Unavailable — mock surface" className={`${control} h-9 border-aurora-accent-primary/45 text-aurora-accent-strong`}><Play className="size-3.5" />New Session</button></div>
        <div className="grid grid-cols-2 border-t border-aurora-border-default/50 bg-[var(--gw0-0_30)] lg:grid-cols-4">{[['Running','2','now'],['Completed','3','today'],['Failed','1','today'],['Median','4m 12s','per session']].map(([k,v,s])=><div key={k} className="border-l border-aurora-border-default/40 px-5 py-3 first:border-l-0"><span className="text-[9.5px] font-bold uppercase tracking-[.12em] text-aurora-text-muted">{k}</span><div className="mt-1"><b className="font-display text-[21px] text-aurora-text-primary">{v}</b><span className="ml-2 text-[10.5px] text-aurora-text-muted">{s}</span></div></div>)}</div>
      </section>
      <div className="flex flex-wrap items-center gap-2"><span className="mr-1 text-[10px] font-bold uppercase tracking-[.14em] text-aurora-text-muted">Sessions</span>{['All','Running','Completed','Failed'].map((label,i)=><button key={label} disabled aria-pressed={i===0} className={`${control} ${i===0?'border-aurora-accent-primary/40 text-aurora-accent-strong':''}`}>{label}</button>)}</div>
      <section className={card}><div className="hidden grid-cols-[90px_minmax(220px,1.4fr)_minmax(150px,1fr)_130px_70px_92px] gap-3 border-b border-aurora-border-strong bg-[var(--gw0-0_48)] px-4 py-2.5 text-[10px] font-bold uppercase tracking-[.13em] text-aurora-text-muted md:grid"><span>Status</span><span>Session</span><span>Loadout</span><span>Container</span><span>Elapsed</span><span /></div>{sessions.map(([status,name,repo,loadout,container,elapsed])=><article key={name} data-mock-region="agents-session" className="grid grid-cols-[1fr_auto] items-start gap-x-3 gap-y-2 border-t border-aurora-border-default/45 px-4 py-3 first:border-t-0 md:grid-cols-[90px_minmax(220px,1.4fr)_minmax(150px,1fr)_130px_70px_92px] md:items-center md:gap-3"><span className={`col-start-2 row-start-1 w-fit justify-self-end rounded-md border px-2 py-1 text-[9.5px] font-bold uppercase md:col-start-auto md:row-start-auto md:justify-self-start ${status==='running'?'border-aurora-success/30 text-aurora-success':status==='failed'?'border-aurora-error/30 text-aurora-error':'border-aurora-border-default text-aurora-text-muted'}`}>{status}</span><div className="col-start-1 row-start-1 min-w-0 md:col-start-auto md:row-start-auto"><h3 className="font-display text-[12.5px] font-bold text-aurora-text-primary md:truncate">{name}</h3><p className="text-[10.5px] text-aurora-text-muted md:truncate">{repo}</p></div><span className="col-start-1 row-start-2 text-[11px] text-aurora-text-muted md:col-start-auto md:row-start-auto"><span className="mr-1 text-[9px] font-bold uppercase tracking-wider text-aurora-text-muted/70 md:hidden">Loadout</span>{loadout}</span><span className="col-start-2 row-start-2 text-right text-[11px] text-aurora-text-muted md:col-start-auto md:row-start-auto md:text-left"><span className="mr-1 text-[9px] font-bold uppercase tracking-wider text-aurora-text-muted/70 md:hidden">Container</span>{container}</span><span className="col-start-1 row-start-3 text-[11px] text-aurora-text-muted md:col-start-auto md:row-start-auto"><span className="mr-1 text-[9px] font-bold uppercase tracking-wider text-aurora-text-muted/70 md:hidden">Elapsed</span>{elapsed}</span><button disabled className={`${control} col-start-2 row-start-3 justify-self-end md:col-start-auto md:row-start-auto`}>{status==='running'?<Square className="size-3" />:<Play className="size-3" />}{status==='running'?'Stop':'Re-run'}</button></article>)}</section>
      <MockNotice noun="Agents" />
    </section>
  </>
}

export function MockTasksPage() {
  return <>
    <AppHeader breadcrumbs={[{ label: 'Tasks' }]} />
    <section data-screen-label="Tasks" data-mock-region="tasks" aria-label="Tasks mock data" className="flex flex-col gap-[14px]">
      <section data-hero="1" className={card}>
        <div className="flex flex-wrap items-end justify-between gap-4 px-6 pt-[22px] pb-4"><div><div className="flex items-center gap-2.5"><span className="text-[10.5px] font-bold uppercase tracking-[.16em] text-aurora-text-muted">Team · Schedules</span><MockSurfaceBadge /></div><div className="mt-2 flex items-center gap-3"><CalendarClock className="size-7 text-aurora-accent-strong" /><h1 className="font-display text-[30px] font-extrabold text-aurora-text-primary">Tasks</h1></div><p className="mt-[7px] max-w-[620px] text-[12.5px] leading-[1.55] text-aurora-text-muted">Recurring agent runs. Each task carries its own loadout, container and repository, and reports back into Activity when it finishes.</p></div><button disabled title="Unavailable — mock surface" className={`${control} h-9 border-aurora-accent-primary/45 text-aurora-accent-strong`}><Play className="size-3.5" />New Task</button></div>
        <div className="grid grid-cols-2 border-t border-aurora-border-default/50 bg-[var(--gw0-0_30)] lg:grid-cols-4">{[['Scheduled','5','tasks'],['Armed','4','live'],['Next Run','02:00','Scope Audit'],['Failures','1','last 7 days']].map(([k,v,s])=><div key={k} className="border-l border-aurora-border-default/40 px-5 py-3 first:border-l-0"><span className="text-[9.5px] font-bold uppercase tracking-[.12em] text-aurora-text-muted">{k}</span><div className="mt-1"><b className="font-display text-[21px] text-aurora-text-primary">{v}</b><span className="ml-2 text-[10.5px] text-aurora-text-muted">{s}</span></div></div>)}</div>
      </section>
      <div className="flex items-center gap-2"><span className="mr-1 text-[10px] font-bold uppercase tracking-[.14em] text-aurora-text-muted">Scheduled</span>{['All','Armed','Paused'].map((label,i)=><button key={label} disabled aria-pressed={i===0} className={`${control} ${i===0?'border-aurora-accent-primary/40 text-aurora-accent-strong':''}`}>{label}</button>)}</div>
      <section className={card}><div className="hidden grid-cols-[54px_minmax(240px,1.5fr)_180px_110px_80px_78px] gap-3 border-b border-aurora-border-strong bg-[var(--gw0-0_48)] px-4 py-2.5 text-[10px] font-bold uppercase tracking-[.13em] text-aurora-text-muted md:grid"><span>On</span><span>Task</span><span>Schedule</span><span>Loadout</span><span>Next</span><span /></div>{tasks.map(([name,desc,schedule,scope,next,armed])=><article key={name} data-mock-region="tasks-row" className="grid grid-cols-[auto_1fr_auto] items-start gap-3 border-t border-aurora-border-default/45 px-4 py-3 first:border-t-0 md:grid-cols-[54px_minmax(240px,1.5fr)_180px_110px_80px_78px] md:items-center"><button disabled role="switch" aria-checked={armed} aria-label="Arm task" className={`relative mt-0.5 h-[19px] w-[34px] rounded-full md:mt-0 ${armed?'bg-aurora-accent-primary':'bg-aurora-border-strong'}`}><span className={`absolute top-0.5 size-[15px] rounded-full bg-aurora-page-bg ${armed?'left-[17px]':'left-0.5'}`} /></button><div><h3 className="font-display text-[12.5px] font-bold text-aurora-text-primary">{name}</h3><p className="text-[10.5px] text-aurora-text-muted">{desc}</p></div><span className={`text-[10.5px] md:order-none ${next==='paused'?'text-aurora-warn':'text-aurora-text-muted'}`}>{next}</span><span className="col-span-2 col-start-2 inline-flex items-center gap-1.5 text-[11px] text-aurora-text-muted md:col-span-1 md:col-start-auto"><Clock3 className="size-3" />{schedule}</span><span className="col-start-2 w-fit rounded-full border border-aurora-accent-primary/25 px-2 py-1 text-[10px] text-aurora-accent-strong md:col-start-auto">{scope}</span><button disabled className={`${control} justify-self-end`}><Play className="size-3" />Run</button></article>)}</section>
      <MockNotice noun="Tasks" />
    </section>
  </>
}
