import { AlertCircle, Braces, Check, FileText, Package, Sparkles } from 'lucide-react'

import { AppHeader } from '@/components/app-header'
import { MockSurfaceBadge } from '@/components/console/mock-surface-badge'

const fieldClass = 'w-full rounded-[10px] border border-aurora-border-default bg-aurora-control-surface px-3 text-[12px] text-aurora-text-primary disabled:cursor-not-allowed disabled:opacity-80'
const toolClass = 'inline-flex h-8 items-center gap-1.5 rounded-[9px] border border-aurora-border-default bg-aurora-control-surface px-3 text-[11px] font-semibold text-aurora-text-muted disabled:cursor-not-allowed disabled:opacity-60'

function FieldStatus({ children, issue = false }: { children: React.ReactNode; issue?: boolean }) {
  return <span className={`inline-flex items-center gap-1 text-[10px] font-semibold ${issue ? 'text-aurora-warn' : 'text-aurora-success'}`}>{issue ? <AlertCircle className="size-3" /> : <Check className="size-3" />}{children}</span>
}

export function MockCreatePage() {
  return <>
    <AppHeader breadcrumbs={[{ label: 'Create' }]} />
    <section data-screen-label="Create" data-mock-region="create" aria-label="Create mock data" className="mx-auto flex w-full max-w-[980px] flex-col gap-[14px]">
      <div className="flex flex-wrap items-center gap-2 rounded-aurora-2 border border-aurora-border-default/50 bg-[var(--gw0-0_38)] p-2.5">
        <button type="button" disabled className={`${toolClass} border-aurora-accent-primary/40 text-aurora-accent-strong`}><FileText className="size-3.5" />Skill</button>
        <button type="button" disabled className={`${toolClass} text-aurora-warn`}><AlertCircle className="size-3.5" />1 issue</button>
        <button type="button" disabled className={toolClass}><Sparkles className="size-3.5" />Writing tips</button>
        <MockSurfaceBadge />
        <span className="flex-1" />
        <div className="inline-flex rounded-[10px] border border-aurora-border-default bg-aurora-control-surface p-1">
          <button type="button" disabled aria-pressed="true" className="h-7 rounded-[7px] bg-[color-mix(in_srgb,var(--aurora-accent-primary)_14%,transparent)] px-3 text-[11px] font-semibold text-aurora-accent-strong"><Package className="mr-1.5 inline size-3" />Artifact</button>
          <button type="button" disabled className="h-7 px-3 text-[11px] font-semibold text-aurora-text-muted">Bundle</button>
        </div>
        <button type="button" disabled title="Unavailable — mock surface" className="inline-flex h-9 cursor-not-allowed items-center gap-2 rounded-[10px] border border-aurora-accent-primary/45 bg-[color-mix(in_srgb,var(--aurora-accent-primary)_12%,transparent)] px-4 text-[12.5px] font-semibold text-aurora-accent-strong opacity-65">Publish</button>
      </div>

      <div className="grid gap-[14px] lg:grid-cols-[minmax(0,1fr)_220px]">
        <section className="overflow-hidden rounded-aurora-2 border border-aurora-border-default/55 bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] shadow-[var(--aurora-shadow-medium),inset_0_1px_0_rgba(255,255,255,0.04)]">
          <div className="grid gap-5 p-5">
            <label className="grid gap-2"><span className="flex items-center justify-between text-[11px] font-bold uppercase tracking-[0.12em] text-aurora-text-muted">Name <FieldStatus>Name reads as a slug</FieldStatus></span><input disabled aria-label="Artifact name" value="repo-triage" className={`${fieldClass} h-10`} /></label>
            <label className="grid gap-2"><span className="flex items-center justify-between text-[11px] font-bold uppercase tracking-[0.12em] text-aurora-text-muted">Description <FieldStatus>Description is loadable</FieldStatus></span><textarea disabled aria-label="Description" value="Cluster open PRs and issues by subsystem, then draft a triage note per cluster." className={`${fieldClass} min-h-20 py-3 leading-[1.55]`} /></label>
            <div className="grid gap-2"><span className="flex items-center justify-between text-[11px] font-bold uppercase tracking-[0.12em] text-aurora-text-muted">Tags <FieldStatus>Tags will scope this artifact</FieldStatus></span><div className="flex flex-wrap gap-2 rounded-[10px] border border-aurora-border-default bg-aurora-control-surface p-2.5"><span className="rounded-full border border-aurora-accent-primary/25 px-2.5 py-1 text-[10.5px] text-aurora-accent-strong">#review</span><span className="rounded-full border border-aurora-accent-primary/25 px-2.5 py-1 text-[10.5px] text-aurora-accent-strong">#github</span><input disabled aria-label="Add a tag" placeholder="add tag…" className="min-w-24 flex-1 bg-transparent text-[11px] outline-none placeholder:text-aurora-text-muted" /></div></div>
            <label className="grid gap-2"><span className="flex items-center justify-between text-[11px] font-bold uppercase tracking-[0.12em] text-aurora-text-muted">Body <FieldStatus>Body has sections and substance</FieldStatus></span><textarea disabled aria-label="Artifact body" value={'## When to use\nInvoke when the user asks to triage, group, or summarize open work in a repository.\n\n## Steps\n1. List open PRs and issues with labels and last activity.\n2. Cluster by touched subsystem, not by label.\n3. For each cluster write: what it is, who owns it, what unblocks it.'} className={`${fieldClass} min-h-[300px] py-4 font-mono leading-[1.7]`} /></label>
          </div>
          <div className="flex items-center gap-2 border-t border-aurora-border-default/50 bg-[var(--gw0-0_30)] px-4 py-2.5 text-[10.5px] text-aurora-text-muted"><MockSurfaceBadge />autosaved 12s ago · illustrative draft</div>
        </section>

        <aside className="flex flex-col gap-3">
          <section className="rounded-aurora-2 border border-aurora-border-default/55 bg-aurora-panel-strong p-3.5"><div className="mb-2 flex items-center gap-2 text-[10px] font-bold uppercase tracking-[0.12em] text-aurora-text-muted"><Braces className="size-3.5" />Insert section</div>{['/ When to use', '/ Steps', '/ Examples', '/ Constraints'].map(label => <button key={label} type="button" disabled className={`${toolClass} mt-1.5 w-full justify-start`}>{label}</button>)}</section>
          <section className="rounded-aurora-2 border border-aurora-warn/30 bg-[color-mix(in_srgb,var(--aurora-warn)_7%,var(--aurora-panel-strong))] p-3.5"><div className="flex items-center gap-2 text-[11px] font-semibold text-aurora-warn"><AlertCircle className="size-3.5" />Frontmatter</div><p className="mt-2 text-[10.5px] leading-[1.5] text-aurora-text-muted">One illustrative publishing issue remains. Validation and publishing are not connected.</p></section>
        </aside>
      </div>
      <div className="flex items-start gap-3 rounded-aurora-1 border border-aurora-warn/30 bg-[color-mix(in_srgb,var(--aurora-warn)_8%,var(--aurora-panel-strong))] px-4 py-3 text-[12px] text-aurora-text-muted"><MockSurfaceBadge className="shrink-0" /><p>This editor reproduces the approved Create mock. Draft content, validation, autosave, and publishing state are illustrative; no controls call a Labby service.</p></div>
    </section>
  </>
}
