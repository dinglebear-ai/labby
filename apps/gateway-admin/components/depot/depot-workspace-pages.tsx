import {
  Bot, Box, CheckCircle2, CirclePlus, Clock3, Container,
  FileCode2, FileText, Inbox, Layers3, Play, Search,
} from 'lucide-react'

import { AppHeader } from '@/components/app-header'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { ConsoleHero } from '@/components/console/console-hero'
import { DashboardPanel } from '@/components/dashboard/panel'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'

const demoArtifacts = [
  ['Skill', 'repo-triage', 'Cluster open PRs and issues, then draft a triage note.', '#review · #github'],
  ['Agent', 'rust-reviewer', 'Review Rust changes and flag unsafe blocks with rationale.', '#rust · #review'],
  ['MCP', 'labby', 'Gateway control plane exposing scoped upstream MCP capabilities.', '#gateway · #mcp'],
  ['Command', '/ship', 'Run release checks and prepare a release draft.', '#release'],
  ['Loadout', 'operator-console', 'Operational tools for logs, services, and infrastructure.', '#ops · #homelab'],
  ['Snippet', 'gateway-reconcile', 'Probe disconnected servers and summarize the delta.', '#gateway'],
]

export function LibraryTabs({ active }: { active: 'artifacts' | 'loadouts' | 'snippets' }) {
  const tabs = [
    ['artifacts', '/library', 'Artifacts'],
    ['loadouts', '/loadouts', 'Loadouts'],
    ['snippets', '/snippets', 'Snippets'],
  ] as const
  return <nav aria-label="Library sections" className="flex gap-6 border-b border-aurora-border-subtle px-3">
    {tabs.map(([id, href, label]) => <a key={id} href={href} aria-current={active === id ? 'page' : undefined} className="border-b-2 border-transparent px-1 py-3 text-sm font-semibold text-aurora-text-muted transition-colors hover:text-aurora-text-primary aria-[current=page]:border-aurora-accent-primary aria-[current=page]:text-aurora-text-primary">{label}</a>)}
  </nav>
}

function PageFrame({ children }: { children: React.ReactNode }) {
  return <div className={`${AURORA_PAGE_SHELL} flex-1`}><div className={`${AURORA_PAGE_FRAME} space-y-4`}>{children}</div></div>
}

export function LibraryPage() {
  return <><AppHeader breadcrumbs={[{ label: 'Depot' }, { label: 'Library' }]} /><PageFrame>
    <LibraryTabs active="artifacts" />
    <ConsoleHero eyebrow="Depot · Library" title="Library" pulse={{ color: 'var(--aurora-warn)', label: 'preview layout' }} actions={<div className="flex gap-2"><Button variant="outline">Backup all</Button><Button><CirclePlus />New loadout</Button></div>} stats={[
      { label: 'Artifacts', value: '102,745', icon: <Box size={12}/> },
      { label: 'Loadouts', value: '4', icon: <Layers3 size={12}/> },
      { label: 'Snippets', value: '6', icon: <FileCode2 size={12}/> },
      { label: 'Authority', value: 'Read only', icon: <CheckCircle2 size={12}/> },
    ]}/>
    <div className="grid gap-4 lg:grid-cols-[210px_1fr]">
      <aside className="space-y-3"><DashboardPanel title="Views"><div className="space-y-1 text-sm"><button className="w-full rounded-aurora-1 bg-aurora-surface-muted px-3 py-2 text-left text-aurora-text-primary">All artifacts</button>{['Published', 'MCP servers', 'Skills', 'Agents', 'Commands'].map(x => <button key={x} className="w-full px-3 py-2 text-left text-aurora-text-muted hover:text-aurora-text-primary">{x}</button>)}</div></DashboardPanel></aside>
      <DashboardPanel title="Artifacts" action={<div className="relative"><Search className="absolute left-3 top-1/2 size-4 -translate-y-1/2 text-aurora-text-muted"/><input aria-label="Filter library" className="h-9 rounded-aurora-1 border border-aurora-border-subtle bg-aurora-panel-low pl-9 pr-3 text-sm" placeholder="Filter artifacts…"/></div>}>
        <div className="divide-y divide-aurora-border-subtle">{demoArtifacts.map(([kind, name, description, tags]) => <div key={name} className="grid gap-2 px-2 py-3 sm:grid-cols-[100px_1fr_180px] sm:items-center"><Badge variant="outline">{kind}</Badge><div><div className="font-semibold text-aurora-text-primary">{name}</div><div className="text-xs text-aurora-text-muted">{description}</div></div><div className="text-xs text-aurora-accent-primary">{tags}</div></div>)}</div>
      </DashboardPanel>
    </div>
  </PageFrame></>
}

export function CreatePage() {
  return <><AppHeader breadcrumbs={[{ label: 'Depot' }, { label: 'Create' }]} /><PageFrame>
    <div className="mx-auto max-w-4xl space-y-4">
      <div className="flex items-center justify-between"><Badge variant="outline">Skill</Badge><div className="flex gap-2"><Badge variant="outline">Draft</Badge><Button>Publish</Button></div></div>
      <section className="min-h-[510px] rounded-aurora-3 border border-aurora-border-subtle bg-aurora-panel-medium p-8 shadow-aurora-panel">
        <textarea aria-label="Artifact name" rows={2} defaultValue="repo-triage" className="w-full resize-none border-0 bg-transparent text-2xl font-semibold leading-tight text-aurora-text-primary outline-none sm:text-3xl"/>
        <textarea aria-label="Artifact description" rows={2} defaultValue="Cluster open PRs and issues, then draft a triage note per cluster." className="mt-2 w-full resize-none border-0 bg-transparent text-sm leading-6 text-aurora-text-muted outline-none"/>
        <div className="my-6 border-t border-aurora-border-subtle"/>
        <textarea aria-label="Artifact content" defaultValue={'## When to use\n\nInvoke when the user asks to triage, group, or summarize open work in a repository.\n\n## Steps\n\n1. List open PRs and issues with labels and last activity.\n2. Cluster by touched subsystem, not by label.\n3. For each cluster write what it is, who owns it, and what unblocks it.'} className="min-h-80 w-full resize-none border-0 bg-transparent font-mono text-sm leading-7 text-aurora-text-primary outline-none"/>
      </section>
    </div>
  </PageFrame></>
}

const agents = [
  ['Running', 'Reconcile the gateway catalog', 'operator-console', 'Claude Code', '12m'],
  ['Running', 'Research Depot publishing contracts', 'research-workbench', 'Codex', '4m'],
  ['Completed', 'Verify Skills build matrix', 'operator-console', 'Codex', '2m'],
  ['Failed', 'Draft plugin manifest', 'research-workbench', 'Claude Code', '41s'],
]

export function AgentsPage() {
  return <><AppHeader breadcrumbs={[{ label: 'Workspace' }, { label: 'Agents' }]} /><PageFrame><ConsoleHero eyebrow="Workspace · Agents" title="Agents" pulse={{ color: 'var(--aurora-warn)', label: 'preview data' }} actions={<Button><CirclePlus/>New session</Button>} stats={[{label:'Running',value:2,icon:<Play size={12}/>},{label:'Completed',value:1,icon:<CheckCircle2 size={12}/>},{label:'Failed',value:1,icon:<Clock3 size={12}/>}]}/><DashboardPanel title="Sessions"><DataTable headings={['Status','Session','Loadout','Harness','Elapsed']} rows={agents}/></DashboardPanel></PageFrame></>
}

const tasks = [
  ['Armed', 'Loadout scope audit', 'Daily · 02:00', 'operator-console', 'in 6h'],
  ['Armed', 'Gateway error digest', 'Mon, Thu · 07:00', 'operator-console', 'in 2d'],
  ['Armed', 'Upstream drift sweep', 'Weekly · Sun 03:00', 'research-workbench', 'in 4d'],
  ['Paused', 'Dependency bump PR', 'Daily · 05:00', 'research-workbench', 'paused'],
]
export function TasksPage() { return <><AppHeader breadcrumbs={[{label:'Workspace'},{label:'Tasks'}]}/><PageFrame><ConsoleHero eyebrow="Workspace · Schedules" title="Tasks" pulse={{color:'var(--aurora-warn)',label:'preview data'}} actions={<Button><CirclePlus/>New task</Button>} stats={[{label:'Scheduled',value:4,icon:<Clock3 size={12}/>},{label:'Armed',value:3,icon:<CheckCircle2 size={12}/>},{label:'Next run',value:'02:00',icon:<Play size={12}/>} ]}/><DashboardPanel title="Scheduled"><DataTable headings={['On','Task','Schedule','Loadout','Next']} rows={tasks}/></DashboardPanel></PageFrame></> }

const files = [
  ['Data','fleet-snapshot.json','stash://me/fleet-snapshot.json','412 KB','2h ago'],
  ['Doc','reconcile-notes.md','stash://me/reconcile-notes.md','18 KB','5h ago'],
  ['Archive','gateway-trace.log','stash://me/gateway-trace.log','96 MB','1d ago'],
  ['Code','schema.prisma','stash://me/schema.prisma','11 KB','1d ago'],
]
export function StashPage() { return <><AppHeader breadcrumbs={[{label:'Workspace'},{label:'Stash'}]}/><PageFrame><ConsoleHero eyebrow="Workspace · Stash" title="Stash" pulse={{color:'var(--aurora-warn)',label:'preview data'}} actions={<Button><CirclePlus/>Upload</Button>} stats={[{label:'Files',value:4,icon:<Inbox size={12}/>},{label:'Size',value:'96.4 MB',icon:<FileText size={12}/>},{label:'Shared',value:2,icon:<Bot size={12}/>} ]}/><div className="rounded-aurora-2 border border-dashed border-aurora-accent-primary/40 p-6 text-center text-sm text-aurora-text-muted">Drop files here to make them available to agents through <code>stash://</code></div><DashboardPanel title="Files"><DataTable headings={['Kind','Name','Address','Size','Added']} rows={files}/></DashboardPanel></PageFrame></> }

const containers = [
  ['Ready','platform-base','Ubuntu 24.04','Node · Python · Rust · Docker','38 pulls'],
  ['Ready','rust-heavy','Debian 12','Rust · PostgreSQL · Docker','12 pulls'],
  ['Building','edge-minimal','Alpine 3.21','Go · Docker · Tailscale','Layer 4/7'],
]
export function DevContainersPage() { return <><AppHeader breadcrumbs={[{label:'Workspace'},{label:'Dev Containers'}]}/><PageFrame><ConsoleHero eyebrow="Workspace · Incus" title="Dev Containers" pulse={{color:'var(--aurora-warn)',label:'preview data'}} actions={<Button><CirclePlus/>New container</Button>}/><div className="grid gap-4 xl:grid-cols-3">{containers.map(([status,name,distro,tools,foot])=><section key={name} className="rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-medium p-5"><div className="flex justify-between"><h2 className="font-semibold text-aurora-text-primary">{name}</h2><Badge variant="outline">{status}</Badge></div><p className="mt-4 text-sm text-aurora-text-muted">{distro}</p><div className="mt-5 flex items-center gap-2 text-xs text-aurora-accent-primary"><Container size={15}/>{tools}</div><p className="mt-5 border-t border-aurora-border-subtle pt-3 text-xs text-aurora-text-muted">{foot}</p></section>)}</div></PageFrame></> }

const logRows = [
  ['Info','gateway','catalog reconciled · 2 healthy upstreams'],
  ['Info','context7','tools/call completed · 200 in 1.7s'],
  ['Warn','claude-macpoo','SSH session reconnected'],
  ['Debug','depot','catalog search returned 50 of 102745'],
  ['Info','labby','skills catalog refreshed'],
]
export function LogsPage() { return <><AppHeader breadcrumbs={[{label:'Logs'}]}/><PageFrame><ConsoleHero eyebrow="Observability" title="Logs" pulse={{color:'var(--aurora-warn)',label:'preview data'}}/><DashboardPanel title="Event stream" action={<div className="flex gap-2"><Button variant="outline">Follow</Button><Button variant="outline">Download</Button></div>}><div className="mb-3 flex gap-2"><div className="relative flex-1"><Search className="absolute left-3 top-1/2 size-4 -translate-y-1/2 text-aurora-text-muted"/><input className="h-9 w-full rounded-aurora-1 border border-aurora-border-subtle bg-aurora-panel-low pl-9 text-sm" placeholder="Filter lines…"/></div><Badge variant="outline">All sources</Badge></div><DataTable headings={['Level','Source','Message']} rows={logRows}/></DashboardPanel></PageFrame></> }

function DataTable({ headings, rows }: { headings: string[]; rows: string[][] }) {
  return <div className="overflow-x-auto"><table className="w-full text-left text-sm"><thead><tr className="border-b border-aurora-border-subtle">{headings.map(h=><th key={h} className="px-3 py-2 text-[11px] uppercase tracking-[.14em] text-aurora-text-muted">{h}</th>)}</tr></thead><tbody>{rows.map((row,index)=><tr key={index} className="border-b border-aurora-border-subtle/70 last:border-0">{row.map((cell,i)=><td key={i} className={`px-3 py-3 ${i === 1 ? 'font-semibold text-aurora-text-primary' : 'text-aurora-text-muted'}`}>{i === 0 ? <Badge variant="outline">{cell}</Badge> : cell}</td>)}</tr>)}</tbody></table></div>
}
