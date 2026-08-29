import { ArrowUpRight, Boxes, Filter, Forklift, Grid2X2, List, Search, Send } from 'lucide-react'

import { AppHeader } from '@/components/app-header'
import { MockSurfaceBadge } from '@/components/console/mock-surface-badge'

const artifacts = [
  ['MCP', 'MCP Registry', 'playwright', 'microsoft · 4h ago', 'Browser automation surface — navigate, snapshot, click and extract with accessibility trees.', '#browser  #testing', '6.2k', '58k', '1240', true],
  ['MCP', 'MCP Registry', 'axon', 'jmagar/axon · 1d ago', 'Self-hosted RAG control plane — crawl, scrape, ingest, embed, search and ask over any corpus.', '#rag  #crawl', '5.1k', '42k', '890', true],
  ['MCP', 'MCP Registry', 'labby', 'jmagar/labby · 6h ago', 'Gateway control plane exposing every downstream MCP server behind one scoped endpoint.', '#gateway  #mcp', '3.3k', '27k', '511', true],
  ['Skill', 'skills.sh', 'repo-triage', 'jmagar · 2d ago', 'Walks open PRs and issues, clusters them by subsystem, and drafts a triage note per cluster.', '#review  #github', '2.4k', '18k', '312', true],
  ['Extension', 'Gemini', 'gemini-docs-ext', 'google · 2d ago', 'Gemini extension exposing internal docs search with citation-grade retrieval.', '#docs  #search', '2.1k', '19k', '260', false],
  ['ACP', 'ACP Registry', 'zed-acp-bridge', 'zed-industries · 12h ago', 'Agent Client Protocol bridge exposing editor context, diagnostics and edits to any agent.', '#editor  #acp', '2.9k', '15k', '233', false],
] as const

const buttonClass = 'inline-flex h-8 items-center gap-1.5 rounded-[9px] border border-aurora-border-default bg-aurora-control-surface px-3 text-[11px] font-semibold text-aurora-text-muted disabled:cursor-not-allowed disabled:opacity-55'

export function MockDiscoveryPage() {
  return <>
    <AppHeader breadcrumbs={[{ label: 'Discovery' }]} />
    <section data-screen-label="Discovery" className="flex flex-col gap-[14px]">
      <section data-hero="1" className="overflow-hidden rounded-aurora-3 border border-[color-mix(in_srgb,var(--aurora-border-default)_45%,var(--aurora-page-bg))] bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] shadow-[var(--aurora-shadow-strong),inset_0_1px_0_rgba(255,255,255,0.05)]">
        <div className="flex flex-wrap items-end justify-between gap-4 px-6 pt-[22px] pb-4">
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-2.5">
              <span className="text-[10.5px] font-bold uppercase tracking-[0.16em] text-aurora-text-muted">Depot · Bazaar</span>
              <span className="inline-flex items-center gap-1.5 text-[10.5px] font-semibold text-aurora-success"><span className="size-1.5 rounded-full bg-aurora-success shadow-[0_0_4px_currentColor]" />9 sources indexed</span>
              <MockSurfaceBadge />
            </div>
            <h1 className="mt-2 font-display text-[30px] leading-[1.04] font-extrabold text-aurora-text-primary">Discovery</h1>
            <p className="mt-[7px] max-w-[600px] text-[12.5px] leading-[1.55] text-aurora-text-muted">Every artifact Depot can reach — registries, marketplaces, catalogs and crawls — searched semantically by Axon and installable in any target format through APM.</p>
            <p className="mt-1 text-[11.5px] font-semibold text-aurora-text-muted">26 indexed · 9 sources · last crawl 4m ago · Mock data</p>
          </div>
          <div className="flex items-center gap-2">
            <button type="button" disabled title="Unavailable — mock surface" className="grid size-8 cursor-not-allowed place-items-center rounded-[9px] border border-aurora-border-default bg-aurora-control-surface text-aurora-text-muted opacity-55"><Boxes className="size-3.5" /></button>
            <button type="button" disabled title="Unavailable — mock surface" className="inline-flex h-9 cursor-not-allowed items-center gap-2 rounded-[10px] border border-aurora-accent-primary/50 bg-[color-mix(in_srgb,var(--aurora-accent-primary)_12%,transparent)] px-4 text-[12.5px] font-semibold text-aurora-accent-strong opacity-65"><ArrowUpRight className="size-3.5" />Publish Artifact</button>
          </div>
        </div>
      </section>

      <div className="flex flex-wrap items-center gap-2 rounded-aurora-2 border border-aurora-border-default/50 bg-[var(--gw0-0_38)] p-2.5" data-mock-region="discovery-controls">
        <label className="relative min-w-[260px] flex-1">
          <Search className="absolute top-1/2 left-3 size-3.5 -translate-y-1/2 text-aurora-text-muted" />
          <input disabled aria-label="Search artifacts" placeholder="Search 26 artifacts — semantic, powered by Axon" className="h-9 w-full cursor-not-allowed rounded-[10px] border border-aurora-border-default bg-aurora-control-surface pr-20 pl-9 text-[12px] text-aurora-text-primary placeholder:text-aurora-text-muted" />
          <span className="absolute top-1/2 right-3 -translate-y-1/2 text-[9.5px] font-bold uppercase tracking-[0.12em] text-aurora-accent-strong">Semantic</span>
        </label>
        <button type="button" disabled className={buttonClass}><Filter className="size-3.5" />Filters</button>
        <button type="button" disabled aria-pressed="true" className={buttonClass}><Grid2X2 className="size-3.5" /></button>
        <button type="button" disabled className={buttonClass}><List className="size-3.5" /></button>
      </div>

      <section data-mock-region="discovery" aria-label="Discovery mock data" className="flex flex-col gap-3">
        <div className="flex flex-wrap items-center gap-1.5">
          {['Trending', 'New', 'Popular', 'Bundled', 'Hot Forks', 'Curated'].map((label, index) => <button key={label} type="button" disabled aria-pressed={index === 0} className={`${buttonClass} ${index === 0 ? 'border-aurora-accent-primary/40 bg-[color-mix(in_srgb,var(--aurora-accent-primary)_12%,transparent)] text-aurora-accent-strong' : ''}`}>{label}</button>)}
          <span className="ml-auto text-[10.5px] text-aurora-text-muted">26 of 26 shown · <MockSurfaceBadge /></span>
        </div>
        <div className="flex items-baseline gap-2"><h2 className="font-display text-[14px] font-bold text-aurora-text-primary">Trending This Week</h2><span className="text-[10.5px] text-aurora-text-muted">velocity of installs + forks</span></div>
        <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
          {artifacts.map(([kind, source, name, publisher, description, tags, stars, installs, forks, installed]) => <article key={name} data-mock-region="discovery-card" className="flex min-h-[236px] flex-col rounded-aurora-2 border border-aurora-border-default/55 bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] p-4 shadow-[var(--aurora-shadow-medium),inset_0_1px_0_rgba(255,255,255,0.04)]">
            <div className="flex items-center justify-between gap-2"><span className="rounded-[5px] border border-aurora-accent-primary/25 bg-[color-mix(in_srgb,var(--aurora-accent-primary)_9%,transparent)] px-2 py-1 text-[9px] font-bold uppercase tracking-[0.1em] text-aurora-accent-strong">{kind}</span><span className="text-[9.5px] text-aurora-text-muted">{source}</span></div>
            <h3 className="mt-3 font-display text-[16px] font-extrabold text-aurora-text-primary">{name}</h3>
            <span className="mt-1 text-[10.5px] text-aurora-text-muted">{publisher}</span>
            <p className="mt-3 flex-1 text-[11.5px] leading-[1.55] text-aurora-text-muted">{description}</p>
            <span className="mt-2 whitespace-pre text-[10px] text-aurora-accent-strong">{tags}</span>
            <div className="mt-3 grid grid-cols-3 border-y border-aurora-border-default/45 py-2 text-center text-[10px] text-aurora-text-muted"><span><b className="block text-[11.5px] text-aurora-text-primary">{stars}</b>Stars</span><span><b className="block text-[11.5px] text-aurora-text-primary">{installs}</b>Installs</span><span><b className="block text-[11.5px] text-aurora-text-primary">{forks}</b>Forks</span></div>
            <div className="mt-3 flex gap-1.5"><button type="button" disabled className={buttonClass}><Forklift className="size-3" />Fork</button><button type="button" disabled className={buttonClass}><Send className="size-3" />Send to Labby</button><button type="button" disabled className={`${buttonClass} ml-auto`}>{installed ? 'In Library' : 'Add'}</button></div>
          </article>)}
        </div>
        <div className="flex items-start gap-3 rounded-aurora-1 border border-aurora-warn/30 bg-[color-mix(in_srgb,var(--aurora-warn)_8%,var(--aurora-panel-strong))] px-4 py-3 text-[12px] text-aurora-text-muted"><MockSurfaceBadge className="shrink-0" /><p>This page reproduces the approved Discovery mock. Artifact listings, counts, sources, and actions are illustrative; no controls call a Labby service.</p></div>
      </section>
    </section>
  </>
}
