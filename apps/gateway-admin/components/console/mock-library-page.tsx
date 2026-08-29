import { Archive, Download, Search, Send, Sparkles } from 'lucide-react'

import { AppHeader } from '@/components/app-header'
import { MockSurfaceBadge } from '@/components/console/mock-surface-badge'

const rows = [
  ['Skill', 'repo-triage', 'Walks open PRs and issues, clusters them by subsystem, and drafts a triage note per cluster.', 'Origin', '2d ago'],
  ['Agent', 'rust-reviewer', 'Reviews Rust diffs against the workspace lint profile and flags unsafe blocks with rationale.', '3 behind', '5h ago'],
  ['MCP', 'axon', 'Self-hosted RAG control plane — crawl, scrape, ingest, embed, search and ask over any corpus.', 'Origin', '1d ago'],
  ['MCP', 'labby', 'Gateway control plane exposing every downstream MCP server behind one scoped endpoint.', 'Origin', '6h ago'],
  ['Command', '/ship', 'Runs the release checklist: changelog, version bump, tag, and a GitHub release draft.', 'Origin', '1w ago'],
  ['Hook', 'cost-ceiling', 'Halts a session when projected token spend crosses the per-run budget.', 'Origin', '9h ago'],
] as const

const pill = 'inline-flex h-8 items-center gap-1.5 rounded-[9px] border border-aurora-border-default bg-aurora-control-surface px-3 text-[11px] font-semibold text-aurora-text-muted disabled:cursor-not-allowed disabled:opacity-60'

export function MockLibraryPage() {
  return <>
    <AppHeader breadcrumbs={[{ label: 'Library' }]} />
    <section data-screen-label="Library" data-mock-region="library" aria-label="Library mock data" className="flex flex-col gap-[14px]">
      <div className="flex flex-wrap items-center gap-2">{[['Artifacts','16'],['Loadouts','4'],['Snippets','6']].map(([label,count],i)=><button key={label} disabled aria-pressed={i===0} className={`${pill} ${i===0?'border-aurora-accent-primary/40 text-aurora-accent-strong':''}`}>{label}<b>{count}</b></button>)}<MockSurfaceBadge /></div>
      <section data-hero="1" className="overflow-hidden rounded-aurora-3 border border-aurora-border-default/50 bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] shadow-[var(--aurora-shadow-strong)]">
        <div className="flex flex-wrap items-end justify-between gap-4 px-6 pt-[22px] pb-4"><div><span className="text-[10.5px] font-bold uppercase tracking-[0.16em] text-aurora-text-muted">Depot · Library</span><h1 className="mt-2 font-display text-[30px] font-extrabold text-aurora-text-primary">Library</h1></div><div className="flex gap-2"><button disabled className={pill}><Archive className="size-3.5" />Backup All</button><button disabled className={`${pill} border-aurora-accent-primary/40 text-aurora-accent-strong`}><Sparkles className="size-3.5" />New Loadout</button></div></div>
        <div className="grid grid-cols-2 border-t border-aurora-border-default/50 bg-[var(--gw0-0_30)] lg:grid-cols-5">{[['Artifacts','16','in library'],['Forks','4','tracking upstream'],['Behind','4','need a merge'],['Public','7','published'],['Loadouts','2','bundled']].map(([k,v,s])=><div key={k} className="border-l border-aurora-border-default/40 px-5 py-3 first:border-l-0"><span className="text-[9.5px] font-bold uppercase tracking-[.12em] text-aurora-text-muted">{k}</span><div className="mt-1"><b className="font-display text-[21px] text-aurora-text-primary">{v}</b><span className="ml-2 text-[10.5px] text-aurora-text-muted">{s}</span></div></div>)}</div>
      </section>
      <div className="flex flex-wrap items-center gap-3 rounded-aurora-1 border border-aurora-warn/30 bg-[color-mix(in_srgb,var(--aurora-warn)_8%,var(--aurora-panel-strong))] px-4 py-3"><span className="text-[12px] font-semibold text-aurora-warn">4 forks are behind upstream.</span><span className="text-[11px] text-aurora-text-muted">Review each diff and merge what you want into your fork.</span><button disabled className={`${pill} ml-auto`}>Review Updates</button><MockSurfaceBadge /></div>
      <div className="flex flex-wrap gap-2">{['All Artifacts 16','Forks 4','Behind Upstream 4','Published 7','Team 7','Private 2'].map((label,i)=><button key={label} disabled aria-pressed={i===0} className={`${pill} ${i===0?'border-aurora-accent-primary/40 text-aurora-accent-strong':''}`}>{label}</button>)}</div>
      <div className="flex flex-wrap items-center gap-2 rounded-aurora-2 border border-aurora-border-default/50 bg-[var(--gw0-0_38)] p-2.5"><Search className="size-3.5 text-aurora-text-muted" /><input disabled aria-label="Filter library" placeholder="Filter 16 artifacts…" className="h-8 min-w-[220px] flex-1 bg-transparent text-[12px] outline-none placeholder:text-aurora-text-muted" /><span className="text-[10.5px] text-aurora-text-muted">16 of 16</span>{['Updated','Name','Kind'].map((label,i)=><button key={label} disabled aria-pressed={i===0} className={pill}>{label}</button>)}</div>
      <section className="overflow-hidden rounded-aurora-2 border border-aurora-border-default/55 bg-aurora-panel-strong">
        <div className="grid grid-cols-[70px_minmax(180px,1fr)_110px_90px_180px] gap-3 border-b border-aurora-border-strong bg-[var(--gw0-0_48)] px-4 py-2.5 text-[10px] font-bold uppercase tracking-[.13em] text-aurora-text-muted"><span>Kind</span><span>Artifact</span><span>Upstream</span><span>Updated</span><span /></div>
        {rows.map(([kind,name,description,upstream,updated])=><article key={name} data-mock-region="library-row" className="grid grid-cols-[70px_minmax(180px,1fr)_110px_90px_180px] items-center gap-3 border-t border-aurora-border-default/45 px-4 py-3 first:border-t-0"><span className="text-[10px] font-bold text-aurora-accent-strong">{kind}</span><div className="min-w-0"><h3 className="font-display text-[12.5px] font-bold text-aurora-text-primary">{name}</h3><p className="truncate text-[10.5px] text-aurora-text-muted">{description}</p></div><span className={upstream.includes('behind')?'text-[10.5px] text-aurora-warn':'text-[10.5px] text-aurora-text-muted'}>{upstream}</span><span className="text-[10.5px] text-aurora-text-muted">{updated}</span><div className="flex justify-end gap-1.5"><button disabled className={pill}><Send className="size-3" />Send to Labby</button><button disabled className={pill}>Edit</button></div></article>)}
        <div className="flex items-center gap-2 border-t border-aurora-border-default/50 bg-[var(--gw0-0_30)] px-4 py-2 text-[10.5px] text-aurora-text-muted"><Download className="size-3" />Illustrative artifact inventory<MockSurfaceBadge className="ml-auto" /></div>
      </section>
      <div className="flex items-start gap-3 rounded-aurora-1 border border-aurora-warn/30 bg-[color-mix(in_srgb,var(--aurora-warn)_8%,var(--aurora-panel-strong))] px-4 py-3 text-[12px] text-aurora-text-muted"><MockSurfaceBadge className="shrink-0" /><p>This page reproduces the approved Library mock. Artifacts, versions, upstream state, and actions are illustrative; no controls call a Labby service.</p></div>
    </section>
  </>
}
