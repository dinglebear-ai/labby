import type { LucideIcon } from 'lucide-react'
import {
  Archive,
  Bot,
  Box,
  Cable,
  Download,
  FolderOpen,
  PackageSearch,
  Plus,
  Search,
  ShieldCheck,
  Sparkles,
} from 'lucide-react'

import { AppHeader } from '@/components/app-header'
import { MockSurfaceBadge } from '@/components/console/mock-surface-badge'

export type MissingMockSurfaceKind =
  | 'discovery'
  | 'create'
  | 'library'
  | 'agents'
  | 'stash'
  | 'containers'
  | 'instance'

type SurfaceSpec = {
  eyebrow: string
  title: string
  description: string
  action: string
  icon: LucideIcon
  stats: Array<[string, string, string]>
  columns: string[]
  rows: string[][]
}

const SURFACES: Record<MissingMockSurfaceKind, SurfaceSpec> = {
  discovery: {
    eyebrow: 'Depot · Bazaar', title: 'Discovery', icon: PackageSearch,
    description: 'Browse MCP servers, skills, commands, hooks, and bundles from connected sources.', action: 'Add source',
    stats: [['Artifacts', '4.2K', '9 sources'], ['MCP servers', '318', '24 updated'], ['Skills', '1.8K', 'curated'], ['Bundles', '126', 'ready']],
    columns: ['Artifact', 'Kind', 'Source', 'Installs', 'Updated'],
    rows: [['filesystem', 'MCP', 'Official', '48.2K', '2d'], ['rust-reviewer', 'Agent', 'tootie.tv', '1.4K', '5h'], ['release-ops', 'Bundle', 'Community', '864', '1d'], ['secrets-sweeper', 'Skill', 'tootie.tv', '2.1K', '3d']],
  },
  create: {
    eyebrow: 'Depot · Authoring', title: 'Create', icon: Plus,
    description: 'Build an artifact or bundle and compile it for every supported agent target.', action: 'Publish',
    stats: [['Flow', 'Artifact', 'selected'], ['Checks', '4/4', 'passing'], ['Targets', '6', 'detected'], ['Visibility', 'Private', 'draft']],
    columns: ['Step', 'Status', 'Output', 'Target', 'Notes'],
    rows: [['Identity', 'Complete', 'release-warden', 'All agents', 'Name and description'], ['Source', 'Complete', 'SKILL.md', 'Codex', 'Validated'], ['Compatibility', 'Complete', '6 targets', 'APM', 'No conflicts'], ['Publish', 'Ready', 'Private', 'Depot', 'Awaiting action']],
  },
  library: {
    eyebrow: 'Depot · Personal', title: 'Library', icon: FolderOpen,
    description: 'Artifacts, loadouts, and snippets installed in this workspace.', action: 'Import',
    stats: [['Artifacts', '18', 'installed'], ['Loadouts', '4', 'active'], ['Snippets', '6', 'ready'], ['Updates', '2', 'available']],
    columns: ['Artifact', 'Kind', 'Version', 'Scope', 'State'],
    rows: [['labby', 'MCP', '1.14.1', 'Personal', 'Current'], ['repo-triage', 'Skill', '2.3.0', 'Personal', 'Update'], ['platform-core', 'Loadout', '—', 'Personal', 'Current'], ['fleet-health', 'Snippet', '—', 'Personal', 'Current']],
  },
  agents: {
    eyebrow: 'Workspace · Agents', title: 'Agents', icon: Bot,
    description: 'Start an agent session on a loadout, development container, and repository.', action: 'New Session',
    stats: [['Running', '2', 'sessions'], ['Waiting', '1', 'session'], ['Containers', '3', 'ready'], ['Repositories', '5', 'bound']],
    columns: ['Session', 'Agent', 'Loadout', 'Container', 'Repository'],
    rows: [['gateway-release', 'Codex', 'project-a-loadout', 'platform-base', 'dinglebear-ai/labby'], ['axon-ingest', 'Claude', 'research', 'rust-heavy', 'dinglebear-ai/axon'], ['docs-sweep', 'Codex', 'platform-core', 'edge-minimal', 'dinglebear-ai/labby']],
  },
  stash: {
    eyebrow: 'Workspace · Files', title: 'Stash', icon: Archive,
    description: 'Files you and your agents keep close across sessions.', action: 'Upload',
    stats: [['Files', '18', 'personal'], ['Storage', '2.4 GB', 'used'], ['Recent', '5', 'this week'], ['Shared', '0', 'personal scope']],
    columns: ['Name', 'Type', 'Owner', 'Size', 'Modified'],
    rows: [['release-notes.md', 'Markdown', 'you', '18 KB', '12m'], ['gateway-audit.json', 'JSON', 'gateway-release', '224 KB', '1h'], ['axon-benchmarks.csv', 'CSV', 'axon-ingest', '1.8 MB', '2d'], ['architecture.png', 'Image', 'you', '4.2 MB', '4d']],
  },
  containers: {
    eyebrow: 'Workspace · Incus', title: 'Dev Containers', icon: Box,
    description: 'System-container images for repeatable agent workspaces.', action: 'New Container',
    stats: [['Images', '3', 'available'], ['Building', '1', 'edge-minimal'], ['Sessions', '2', 'attached'], ['Cache', '18 GB', 'warm']],
    columns: ['Image', 'Base', 'Toolchain', 'Used by', 'State'],
    rows: [['platform-base', 'Ubuntu 24.04', 'Node · Rust', '1 session', 'Ready'], ['rust-heavy', 'Debian 12', 'Rust · sccache', '1 session', 'Ready'], ['edge-minimal', 'Alpine 3.21', 'Minimal', '—', 'Building']],
  },
  instance: {
    eyebrow: 'Control Plane · Hosted', title: 'Labby Instance', icon: Cable,
    description: 'Hosted Labby capacity, access, deployment, and workspace health.', action: 'Manage',
    stats: [['Region', 'eu-west', 'hosted'], ['Seats', '9/25', 'active'], ['Uptime', '99.98%', '30 days'], ['Version', '1.14.1', 'current']],
    columns: ['Service', 'Region', 'Replicas', 'Version', 'Status'],
    rows: [['Gateway', 'eu-west', '2', '1.14.1', 'Healthy'], ['Runner pool', 'eu-west', '4', '1.14.1', 'Healthy'], ['Artifact cache', 'eu-west', '2', '1.14.1', 'Healthy'], ['Web console', 'global', '3', '0.30.0', 'Healthy']],
  },
}

const CARD = 'overflow-hidden rounded-aurora-2 border border-[color-mix(in_srgb,var(--aurora-border-default)_45%,var(--aurora-page-bg))] bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] shadow-[var(--aurora-shadow-medium),inset_0_1px_0_rgba(255,255,255,0.04)]'

export function MockMissingSurfacePage({ kind }: { kind: MissingMockSurfaceKind }) {
  const spec = SURFACES[kind]
  const Icon = spec.icon
  return (
    <>
      <AppHeader breadcrumbs={[{ label: spec.title }]} />
      <section data-screen-label={spec.title} className="flex flex-col gap-[14px]">
        <section data-hero="1" className="overflow-hidden rounded-aurora-3 border border-[color-mix(in_srgb,var(--aurora-border-default)_45%,var(--aurora-page-bg))] bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] shadow-[var(--aurora-shadow-strong),inset_0_1px_0_rgba(255,255,255,0.05)]">
          <div className="flex flex-wrap items-end justify-between gap-4 px-6 pt-[22px] pb-[18px]">
            <div className="min-w-0">
              <div className="flex flex-wrap items-center gap-2.5"><span className="text-[10.5px] font-bold uppercase tracking-[0.16em] text-aurora-text-muted">{spec.eyebrow}</span><MockSurfaceBadge /></div>
              <div className="mt-2 flex items-center gap-3"><Icon className="size-7 text-aurora-accent-strong" strokeWidth={1.6} /><h1 className="font-display text-[30px] leading-[1.04] font-extrabold text-aurora-text-primary">{spec.title}</h1></div>
              <p className="mt-[7px] max-w-[620px] text-[12.5px] leading-[1.55] text-aurora-text-muted">{spec.description}</p>
            </div>
            <button type="button" disabled title="Unavailable — mock surface" className="inline-flex h-9 cursor-not-allowed items-center gap-2 rounded-[10px] border border-aurora-border-default bg-aurora-control-surface px-4 text-[13px] font-semibold text-aurora-text-muted opacity-65"><Sparkles className="size-3.5" />{spec.action}</button>
          </div>
          <div className="grid grid-cols-2 border-t border-aurora-border-default/55 bg-[var(--gw0-0_30)] lg:grid-cols-4">
            {spec.stats.map(([label, value, sub]) => <div key={label} className="border-l border-aurora-border-default/40 px-5 py-3 first:border-l-0"><div className="text-[9.5px] font-bold uppercase tracking-[0.12em] text-aurora-text-muted">{label}</div><div className="mt-1 flex items-baseline gap-2"><span className="font-display text-[21px] font-extrabold text-aurora-text-primary">{value}</span><span className="text-[10.5px] text-aurora-text-muted">{sub}</span></div></div>)}
          </div>
        </section>

        <div className="flex items-start gap-3 rounded-aurora-1 border border-aurora-warn/30 bg-[color-mix(in_srgb,var(--aurora-warn)_8%,var(--aurora-panel-strong))] px-4 py-3 text-[12px] leading-[1.55] text-aurora-text-muted"><MockSurfaceBadge className="mt-0.5 shrink-0" /><p>This page reproduces the approved mock only. The data below is illustrative and no controls call a Labby service.</p></div>

        <section className={CARD} data-mock-region={kind} aria-label={`${spec.title} mock data`}>
          <div className="flex items-center gap-2 border-b border-aurora-border-default/70 bg-[var(--gw0-0_38)] px-4 py-2.5"><Search className="size-3.5 text-aurora-text-muted" /><span className="text-[10px] font-bold uppercase tracking-[0.14em] text-aurora-text-muted">{spec.title}</span><span className="ml-auto text-[10.5px] text-aurora-text-muted">{spec.rows.length} illustrative rows</span><MockSurfaceBadge /></div>
          <div className="aurora-scrollbar overflow-x-auto">
            <div className="min-w-[820px]">
              <div className="grid grid-cols-5 gap-4 border-b border-aurora-border-strong bg-[var(--gw0-0_48)] px-4 py-2.5 text-[10px] font-bold uppercase tracking-[0.14em] text-aurora-text-muted">{spec.columns.map(column => <span key={column}>{column}</span>)}</div>
              {spec.rows.map((row, index) => <div key={row[0]} className="grid grid-cols-5 gap-4 border-t border-aurora-border-default/55 px-4 py-3 text-[11.5px] first:border-t-0 hover:bg-aurora-hover-bg">{row.map((cell, cellIndex) => <span key={`${index}-${cellIndex}`} className={cellIndex === 0 ? 'font-display font-bold text-aurora-text-primary' : 'truncate text-aurora-text-muted'} title={cell}>{cell}</span>)}</div>)}
            </div>
          </div>
          <div className="flex items-center gap-2 border-t border-aurora-border-default/70 bg-[var(--gw0-0_30)] px-4 py-2 text-[10.5px] text-aurora-text-muted"><ShieldCheck className="size-3" />No actions are connected on this mock surface<span className="ml-auto inline-flex items-center gap-1"><Download className="size-3" />Static preview</span></div>
        </section>
      </section>
    </>
  )
}
