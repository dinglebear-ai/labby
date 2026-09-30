'use client'

import { ArrowLeftRight, Check, X } from 'lucide-react'
import type { FederatedArtifact } from '@/lib/api/depot-client'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { artifactTitle } from './discover-model'

const BUNDLE_CONTENTS: Record<string, Record<string, string[]>> = {
  'project-a-loadout': { Skill:['repo-triage','changelog-writer','doc-crawler','secrets-sweeper'], Agent:['rust-reviewer'], MCP:['labby','corpus','playwright'], Command:['/ship'], Hook:[], Prompt:[] },
  'oncall-loadout': { Skill:['doc-crawler'], Agent:['rust-reviewer'], MCP:['labby','corpus'], Command:['/scope-audit'], Hook:['cost-ceiling','pre-commit-guard'], Prompt:['incident-postmortem'] },
  'homelab-pack': { Skill:['repo-triage','secrets-sweeper','doc-crawler'], Agent:[], MCP:['unraid-ops'], Command:['/ship','/scope-audit'], Hook:[], Prompt:[] },
  'review-suite': { Skill:['repo-triage'], Agent:['rust-reviewer'], MCP:[], Command:[], Hook:['pre-commit-guard'], Prompt:[] },
}

const KIND_ORDER = ['Agent','Skill','Command','Hook','MCP','Prompt'] as const
export type BundleCompareRow = { kind:string; name:string; inA:boolean; inB:boolean }

export function canCompareBundles(artifacts: readonly FederatedArtifact[]) {
  return artifacts.length === 2 && artifacts.every(artifact => Boolean(BUNDLE_CONTENTS[artifactTitle(artifact)]))
}

export function bundleCompareRows(a: FederatedArtifact, b: FederatedArtifact): BundleCompareRow[] {
  const aContents=BUNDLE_CONTENTS[artifactTitle(a)] ?? {}, bContents=BUNDLE_CONTENTS[artifactTitle(b)] ?? {}
  return KIND_ORDER.flatMap(kind => {
    const left=aContents[kind] ?? [], right=bContents[kind] ?? []
    const union=[...left,...right.filter(name=>!left.includes(name))]
    return union.map(name=>({kind,name,inA:left.includes(name),inB:right.includes(name)}))
  })
}

export function DiscoverBundleCompare({ open, artifacts, onOpenChange }:{ open:boolean; artifacts:FederatedArtifact[]; onOpenChange:(open:boolean)=>void }) {
  const [a,b]=artifacts
  if (!a || !b) return null
  const rows=bundleCompareRows(a,b)
  return <Dialog open={open} onOpenChange={onOpenChange}>
    <DialogContent aria-label="Compare bundles" className="max-h-[82svh] max-w-[780px] overflow-hidden border-aurora-border-strong bg-aurora-panel-strong p-0 text-aurora-text-primary">
      <DialogHeader className="border-b border-aurora-border-subtle bg-[var(--gw0-0_38)] px-5 py-4">
        <div className="flex items-center gap-2"><ArrowLeftRight className="size-4 text-aurora-accent-strong"/><DialogTitle className="font-display text-lg">{artifactTitle(a)} vs {artifactTitle(b)}</DialogTitle></div>
        <DialogDescription>Compare the known fixture contents of these selected bundles. This view does not mutate either artifact.</DialogDescription>
      </DialogHeader>
      <div className="overflow-auto p-4">
        <div className="grid min-w-[560px] grid-cols-[90px_minmax(180px,1fr)_120px_120px] items-center border-b border-aurora-border-strong px-2 pb-2 text-[9.5px] font-bold uppercase tracking-[0.12em] text-aurora-text-muted"><span>Kind</span><span>Artifact</span><span className="truncate text-center">{artifactTitle(a)}</span><span className="truncate text-center">{artifactTitle(b)}</span></div>
        {rows.map(row=><div key={row.kind+':'+row.name} className="grid min-w-[560px] grid-cols-[90px_minmax(180px,1fr)_120px_120px] items-center border-b border-aurora-border-subtle/70 px-2 py-2 text-xs"><span className="font-bold uppercase tracking-[0.08em] text-aurora-text-muted">{row.kind}</span><span className="truncate font-medium text-aurora-text-primary">{row.name}</span><CompareMark on={row.inA}/><CompareMark on={row.inB}/></div>)}
      </div>
      <div className="flex justify-end border-t border-aurora-border-subtle px-4 py-3"><Button variant="outline" size="sm" onClick={()=>onOpenChange(false)}><X className="size-3.5"/>Close</Button></div>
    </DialogContent>
  </Dialog>
}

function CompareMark({on}:{on:boolean}) {
  return <span className={`mx-auto grid size-5 place-items-center rounded-full border ${on?'border-aurora-success/40 bg-aurora-success/10 text-aurora-success':'border-aurora-border-default bg-aurora-control-surface text-aurora-text-muted/50'}`}>{on?<Check className="size-3"/>:<span aria-hidden>—</span>}<span className="sr-only">{on?'Included':'Not included'}</span></span>
}
