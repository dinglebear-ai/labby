'use client'

import * as React from 'react'
import {
  Check, ChevronDown, CircleAlert, CircleCheck, Clipboard,
  FileType2, Lock, MoreHorizontal, RotateCcw, Settings2,
} from 'lucide-react'

import { Button, buttonVariants } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { AppHeader } from '@/components/app-header'
import { ConsoleHero } from '@/components/console/console-hero'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel,
  DropdownMenuSeparator, DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import {
  ARTIFACT_KINDS, composeArtifactSource,
  type ArtifactKind, type ArtifactMetadata, validateArtifactDraft,
} from '@/lib/editor/artifact-standards'
import { cn } from '@/lib/utils'
import { toast } from 'sonner'

const STARTER_BODY = `## When to use

Invoke when the user asks to triage, group, or summarize open work in a repository.

## Steps

1. List open PRs and issues with labels and last activity.
2. Cluster by touched subsystem, not by label.
3. For each cluster write what it is, who owns it, and what unblocks it.`

const STARTER_METADATA: ArtifactMetadata = {
  name: 'repo-triage',
  description: 'Cluster open PRs and issues, then draft a triage note per cluster.',
  license: '',
  compatibility: '',
  allowedTools: '',
}

export function ArtifactComposer() {
  const [kind, setKind] = React.useState<ArtifactKind>('Skill')
  const [metadata, setMetadata] = React.useState<ArtifactMetadata>(STARTER_METADATA)
  const [content, setContent] = React.useState(STARTER_BODY)
  const [frontmatterOpen, setFrontmatterOpen] = React.useState(false)
  const [workspaceMode, setWorkspaceMode] = React.useState<'artifact' | 'bundle'>('artifact')

  const source = React.useMemo(() => composeArtifactSource(kind, metadata, content), [content, kind, metadata])
  const issues = React.useMemo(() => validateArtifactDraft(kind, metadata, content), [content, kind, metadata])
  const errors = issues.filter((entry) => entry.severity === 'error')

  const updateMetadata = (field: keyof ArtifactMetadata) => (value: string) => setMetadata((current) => ({ ...current, [field]: value }))
  const copySource = React.useCallback(async () => {
    try {
      await navigator.clipboard.writeText(source)
      toast.success('Complete source copied')
    } catch {
      toast.error('Could not copy complete source')
    }
  }, [source])
  const append = (snippet: string) => setContent((current) => `${current.replace(/\s+$/, '')}\n\n${snippet}\n`)
  const reset = () => {
    setKind('Skill')
    setMetadata(STARTER_METADATA)
    setContent(STARTER_BODY)
  }

  const headerActions = <div className="flex items-center gap-1.5">
        <DropdownMenu>
          <Tooltip><TooltipTrigger asChild><DropdownMenuTrigger asChild><Button variant="outline" size="icon" aria-label={`Artifact type: ${kind}`}><FileType2 /></Button></DropdownMenuTrigger></TooltipTrigger><TooltipContent sideOffset={7}>Artifact type · {kind}</TooltipContent></Tooltip>
          <DropdownMenuContent align="start" className="min-w-48 border-aurora-border-strong bg-aurora-panel-strong">
            <DropdownMenuLabel>Artifact type</DropdownMenuLabel><DropdownMenuSeparator />
            {ARTIFACT_KINDS.map((option) => <DropdownMenuItem key={option} onSelect={() => setKind(option)}>{option === kind ? <Check /> : <span className="size-4" />}{option}</DropdownMenuItem>)}
          </DropdownMenuContent>
        </DropdownMenu>
        <Badge variant="outline" className="h-8 gap-1.5 border-aurora-warn/45 text-aurora-warn"><Lock className="size-3" />Publishing unavailable</Badge>
        <DropdownMenu>
          <Tooltip><TooltipTrigger asChild><DropdownMenuTrigger asChild><Button size="icon" variant="outline" aria-label="More artifact actions"><MoreHorizontal /></Button></DropdownMenuTrigger></TooltipTrigger><TooltipContent sideOffset={7}>More actions</TooltipContent></Tooltip>
          <DropdownMenuContent align="end" className="min-w-52 border-aurora-border-strong bg-aurora-panel-strong">
            <DropdownMenuLabel>Artifact actions</DropdownMenuLabel><DropdownMenuSeparator />
            <DropdownMenuItem onSelect={() => void copySource()}><Clipboard />Copy complete source</DropdownMenuItem>
            <DropdownMenuItem onSelect={() => setFrontmatterOpen(true)}><Settings2 />Edit frontmatter</DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={reset}><RotateCcw />Restore starter</DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

  const bundleGroups = [['Agent',['rust-reviewer']],['Command',['/ship','/scope-audit']],['Skill',['repo-triage','changelog-writer']],['Hook',['pre-commit-guard']],['MCP',['labby','axon']],['Prompt',[]]] as const
  return <>
    <AppHeader breadcrumbs={[{ label: 'Depot', href: '/depot/' }, { label: 'Create' }]} actions={headerActions} />
    <div className={cn(AURORA_PAGE_SHELL, 'flex-1')}><div className={cn(AURORA_PAGE_FRAME, 'min-h-[calc(100vh-5.5rem)]')}>
      <ConsoleHero
        eyebrow="Depot · Studio"
        title="Create artifact"
        pulse={{ color: 'var(--aurora-warn)', label: 'read-only preview' }}
        stats={[
          { label: 'Kind', value: kind, icon: <FileType2 className="size-3" /> },
          { label: 'Workspace', value: workspaceMode === 'artifact' ? 'Artifact' : 'Bundle', icon: <Settings2 className="size-3" /> },
          { label: 'Validation', value: errors.length ? 'Needs work' : 'Passing', icon: errors.length ? <CircleAlert className="size-3" /> : <CircleCheck className="size-3" />, tone: errors.length ? 'var(--aurora-warn)' : 'var(--aurora-success)' },
        ]}
      />
      <div className="mx-auto mt-4 max-w-5xl">
        <div className="mb-3 flex flex-wrap items-center justify-between gap-3">
          <DropdownMenu><DropdownMenuTrigger className={buttonVariants({ variant: 'outline' })}><FileType2/>{kind}<ChevronDown/></DropdownMenuTrigger><DropdownMenuContent>{ARTIFACT_KINDS.map((option)=><DropdownMenuItem key={option} onSelect={()=>setKind(option)}>{option===kind?<Check/>:<span className="size-4"/>}{option}</DropdownMenuItem>)}</DropdownMenuContent></DropdownMenu>
          <div className="flex items-center gap-2"><Badge variant="outline" className={issues.length?'border-aurora-warn/50 text-aurora-warn':'border-aurora-success/50 text-aurora-success'}>{issues.length} issue{issues.length===1?'':'s'}</Badge><div className="flex rounded-full border border-aurora-border-subtle bg-aurora-control-surface p-0.5"><button onClick={()=>setWorkspaceMode('artifact')} className={cn('rounded-full px-4 py-1.5 text-xs font-semibold',workspaceMode==='artifact'?'bg-aurora-selected-bg text-aurora-accent-primary':'text-aurora-text-muted')}>Artifact</button><button onClick={()=>setWorkspaceMode('bundle')} className={cn('rounded-full px-4 py-1.5 text-xs font-semibold',workspaceMode==='bundle'?'bg-aurora-selected-bg text-aurora-accent-primary':'text-aurora-text-muted')}>Bundle</button></div></div>
        </div>
        <div role="status" className="mb-4 flex items-start gap-2 rounded-aurora-2 border border-aurora-warn/35 bg-aurora-warn/5 px-4 py-3 text-xs leading-5 text-aurora-text-muted"><Lock className="mt-0.5 size-4 shrink-0 text-aurora-warn"/><span><strong className="text-aurora-text-primary">Read-only authoring preview.</strong> Depot publishing and compilation are unavailable until delegated mutation authority is negotiated. You can edit and copy the complete source without changing Depot.</span></div>
        {workspaceMode==='artifact'?<div className="grid items-start gap-4 lg:grid-cols-[minmax(0,1fr)_270px]">
          <section className="min-h-[680px] rounded-aurora-3 border border-aurora-border-strong bg-aurora-panel-medium p-8 shadow-aurora-panel">
            <input aria-label="Artifact name" value={metadata.name} onChange={(event)=>updateMetadata('name')(event.target.value)} className="w-full bg-transparent text-3xl font-bold text-aurora-text-primary outline-none"/>
            <textarea aria-label="Artifact description" value={metadata.description} onChange={(event)=>updateMetadata('description')(event.target.value)} rows={2} className="mt-3 w-full resize-none bg-transparent text-sm leading-6 text-aurora-text-muted outline-none"/>
            <div className="mt-2 flex gap-2"><Badge variant="outline" className="text-aurora-accent-primary">#review</Badge><Badge variant="outline" className="text-aurora-accent-primary">#github</Badge></div>
            <div className="my-6 border-t border-aurora-border-subtle"/>
            <textarea aria-label="Artifact content" value={content} onChange={(event)=>setContent(event.target.value)} className="min-h-[430px] w-full resize-none bg-transparent font-mono text-sm leading-8 text-aurora-text-primary outline-none"/>
            <div className="mt-4 flex flex-wrap items-center gap-2 border-t border-aurora-border-subtle pt-4">{['When to use','Steps','Examples','Constraints'].map((item)=><button key={item} onClick={()=>append(`## ${item}`)} className="rounded-full border border-aurora-border-subtle px-3 py-1 text-xs text-aurora-text-muted hover:text-aurora-text-primary">/ {item}</button>)}<button onClick={()=>setFrontmatterOpen(!frontmatterOpen)} className="ml-auto text-xs text-aurora-text-muted">Frontmatter</button></div>
          </section>
          <aside className="overflow-hidden rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-medium"><div className="border-b border-aurora-border-subtle px-4 py-3 text-[10px] font-bold uppercase tracking-[.16em] text-aurora-text-muted">Writing a {kind.toLowerCase()}</div>{[['Name is a slug',Boolean(metadata.name)],['Description is loadable',Boolean(metadata.description)],['At least two tags',true],['Body has sections',content.includes('## ')],['Body has substance',content.length>160],['Example transcript',content.includes('Example')]].map(([label,ok])=><div key={String(label)} className="flex gap-3 border-b border-aurora-border-subtle px-4 py-3"><span className={ok?'text-aurora-success':'text-aurora-warn'}>{ok?<CircleCheck className="size-4"/>:<CircleAlert className="size-4"/>}</span><div><strong className="block text-xs text-aurora-text-primary">{label}</strong><span className="text-[11px] text-aurora-text-muted">{ok?'Looks good.':'Optional — this makes the artifact easier to reuse.'}</span></div></div>)}</aside>
        </div>:<section className="rounded-aurora-3 border border-aurora-border-strong bg-aurora-panel-medium p-8 shadow-aurora-panel"><h1 className="text-3xl font-bold">{metadata.name}</h1><div className="mt-4 flex flex-wrap gap-2 text-xs text-aurora-text-muted"><span className="font-bold uppercase tracking-wider">Potential targets</span>{['Loadout','Claude plugin.json','marketplace.json','gemini-extension.json','Agent Plugins','ARD ai-catalog.json'].map((item)=><Badge key={item} variant="outline">{item}</Badge>)}</div><div className="mt-7 grid gap-3 md:grid-cols-2 lg:grid-cols-3">{bundleGroups.map(([group,items])=><div key={group} className="min-h-36 rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-low p-3"><div className="flex justify-between text-xs font-bold uppercase tracking-wider text-aurora-accent-primary"><span>{group}</span><span>{items.length}</span></div><div className="mt-3 space-y-2">{items.map((item)=><div key={item} className="rounded-aurora-1 bg-aurora-control-surface px-3 py-2 text-sm"><span>{item}</span></div>)}</div></div>)}</div><p className="mt-5 border-t border-aurora-border-subtle pt-4 text-right text-xs text-aurora-text-muted">Bundle editing and compilation require Depot mutation authority.</p></section>}
      </div>
    </div></div>
  </>
}
