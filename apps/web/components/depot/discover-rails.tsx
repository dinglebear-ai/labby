'use client'

import Link from 'next/link'
import { Download } from 'lucide-react'
import type { FederatedArtifact } from '@/lib/api/depot-client'
import { mockDepotLibraryArtifactIds } from '@/lib/api/depot-mock-data'
import { artifactKind, artifactTitle } from './discover-model'
import { discoverKindPresentation, DiscoverPublisherVerifiedIcon } from './discover-kind-presentation'

type Rail = { label: string; hint: string; items: FederatedArtifact[] }

function newest(a: FederatedArtifact, b: FederatedArtifact) {
  return Date.parse(b.currentRevision?.authoredAt ?? '') - Date.parse(a.currentRevision?.authoredAt ?? '')
}

export function discoverReferenceRails(artifacts: FederatedArtifact[]): Rail[] {
  return [
    { label: 'Popular This Week', hint: 'installs, last 7 days', items: [...artifacts].sort((a,b)=>(b.metrics?.installs??-1)-(a.metrics?.installs??-1)).slice(0,8) },
    { label: 'New From Your Team', hint: 'tootie.tv + jmagar', items: artifacts.filter(a=>a.namespace==='tootie.tv'||a.namespace==='jmagar').sort(newest).slice(0,8) },
    { label: 'Pairs With Your Loadouts', hint: 'co-installed with what you run', items: artifacts.filter(a=>!mockDepotLibraryArtifactIds.has(a.artifactId)).slice(0,8) },
  ]
}

export function DiscoverRails({ artifacts, artifactHref, unavailableReason }: { artifacts: FederatedArtifact[]; artifactHref: (providerId?: string, id?: string) => string; unavailableReason?: string }) {
  return <div data-discover-rails="1" className="flex flex-col gap-3">{(artifacts.length ? discoverReferenceRails(artifacts) : [
    { label: 'Popular This Week', hint: 'installs, last 7 days', items: [] },
    { label: 'New From Your Team', hint: 'team publishers', items: [] },
    { label: 'Pairs With Your Loadouts', hint: 'co-installed with what you run', items: [] },
  ] satisfies Rail[]).map(rail=><section key={rail.label} className="flex min-w-0 flex-col gap-2"><div className="flex h-[22px] items-baseline gap-[9px] px-0.5"><h2 className="font-display text-[16px] font-extrabold leading-[normal] tracking-[-0.012em] text-aurora-text-primary">{rail.label}</h2><span className="text-[11.5px] text-aurora-text-muted">{rail.hint}</span></div><div className="aurora-scrollbar flex gap-[11px] overflow-x-auto overflow-y-hidden px-0.5 pb-[7px] pt-px">{rail.items.length===0&&unavailableReason?<div className="flex w-[264px] shrink-0 items-center rounded-aurora-2 border border-dashed border-aurora-border-strong/60 bg-aurora-panel-medium px-[15px] py-[13px] text-[11.5px] leading-relaxed text-aurora-text-muted">{unavailableReason}</div>:null}{rail.items.map(artifact=>{ const presentation=discoverKindPresentation(artifactKind(artifact)); const Icon=presentation.icon; return <Link key={artifact.providerId+':'+artifact.artifactId} href={artifactHref(artifact.providerId,artifact.artifactId)} data-hovercard="1" className="relative flex w-[264px] shrink-0 flex-col gap-[9px] rounded-aurora-2 border border-[color-mix(in_srgb,var(--aurora-border-default)_45%,var(--aurora-page-bg))] bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] px-[15px] py-[13px] text-left shadow-[var(--aurora-shadow-medium),inset_0_1px_0_rgba(255,255,255,0.04)] transition-colors hover:bg-aurora-hover-bg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary"><span aria-hidden className="absolute bottom-3.5 left-0 top-3.5 w-0.5" style={{background:`color-mix(in srgb, ${presentation.tone} 60%, transparent)`}}/><span className="flex min-w-0 items-center gap-[9px]"><span aria-hidden className="grid size-[34px] shrink-0 place-items-center rounded-[10px] border" style={{color:presentation.color,borderColor:`color-mix(in srgb, ${presentation.tone} 28%, transparent)`,background:`color-mix(in srgb, ${presentation.tone} 9%, transparent)`}}><Icon className="size-[18px]"/></span><span className="min-w-0 flex flex-col gap-0.5"><span className="flex min-w-0 items-center gap-1.5"><span className="truncate font-display text-sm font-[760] leading-[18.25px] text-aurora-text-primary">{artifactTitle(artifact)}</span>{artifact.publisherVerified===true?<DiscoverPublisherVerifiedIcon aria-label="Publisher verified" className="size-[11px] shrink-0 text-aurora-success"/>:null}</span><span className="truncate text-[10.5px] font-[650] uppercase tracking-[0.06em]" style={{color:presentation.color}}>{artifactKind(artifact)}</span></span></span><span className="line-clamp-2 min-h-[34px] text-[11.5px] leading-[1.5] text-aurora-text-muted">{artifact.description??artifact.descriptor?.description??'No description supplied.'}</span><span className="flex min-w-0 items-center gap-[7px] border-t border-[color-mix(in_srgb,var(--aurora-border-default)_45%,transparent)] pt-[7px]"><span className="min-w-0 flex-1 truncate text-[10.5px] leading-[14px] text-aurora-text-muted">{artifact.namespace??'Publisher not supplied'}</span>{Number.isSafeInteger(artifact.metrics?.installs)&&artifact.metrics!.installs!>=0?<span title="Installs" className="inline-flex shrink-0 items-center gap-1 text-[10.5px] font-[650] leading-[14px] tabular-nums text-aurora-text-muted"><Download className="size-[11px]"/>{new Intl.NumberFormat('en',{notation:'compact',maximumFractionDigits:1}).format(artifact.metrics!.installs!)}</span>:null}</span></Link>})}</div></section>)}</div>
}
