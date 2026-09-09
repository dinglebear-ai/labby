'use client'

import * as React from 'react'
import {
  CircleAlert, CircleCheck, Clipboard, ShieldCheck,
  MoreHorizontal, RotateCcw, Settings2, CircleHelp, PencilLine, X,
} from 'lucide-react'

import { Button } from '@/components/ui/button'
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
  composeArtifactSource, artifactPath,
  type ArtifactKind, type ArtifactMetadata, validateArtifactDraft,
} from '@/lib/editor/artifact-standards'
import { cn } from '@/lib/utils'
import { toast } from 'sonner'
import { consumeOwnerLinkApproval, depotPublishCapability, publishDepotSkill, type DepotPublishCapability } from '@/lib/api/depot-client'
import { getBrowserSessionEpoch, subscribeToBrowserSession } from '@/lib/auth/session-store'
import { ArtifactFrontmatterPreview } from './artifact-frontmatter-preview'
import { ArtifactValidationPanel } from './artifact-validation-panel'
import { ArtifactWritingExample } from './artifact-writing-example'
import { ArtifactFormattingToolbar } from './artifact-formatting-toolbar'
import { formatArtifactSelection } from '@/lib/editor/artifact-formatting'
import { artifactLanguage } from '@/lib/editor/artifact-standards'
import { SafeMarkdown } from '@/components/markdown/safe-markdown'
import { ArtifactWorkspaceSwitch } from './artifact-workspace-switch'
import { ArtifactKindPicker } from './artifact-kind-picker'
import { ArtifactDescriptionField } from './artifact-description-field'
import { ArtifactFieldIndicator } from './artifact-field-indicator'
import { artifactValidationSummary, skillAuthoringSummary } from '@/lib/editor/artifact-validation-summary'

const STARTER_BODY = `## When to use

Invoke when the user asks to triage, group, or summarize open work in a repository.

## Steps

1. List open PRs and issues with labels and last activity.
2. Cluster by touched subsystem, not by label.
3. For each cluster write: what it is, who owns it, what unblocks it.`

const STARTER_METADATA: ArtifactMetadata = {
  name: 'repo-triage',
  description: 'Cluster open PRs and issues by subsystem, then draft a triage note per cluster.',
  tags: ['review', 'github'],
  license: '',
  compatibility: '',
  allowedTools: '',
}

export function ArtifactComposer() {
  const [kind, setKind] = React.useState<ArtifactKind>('Skill')
  const [metadata, setMetadata] = React.useState<ArtifactMetadata>(STARTER_METADATA)
  const [content, setContent] = React.useState(STARTER_BODY)
  const [tagInput, setTagInput] = React.useState('')
  const contentRef = React.useRef<HTMLTextAreaElement>(null)
  const gutterRef = React.useRef<HTMLDivElement>(null)
  const [documentView, setDocumentView] = React.useState<'source' | 'preview'>('source')
  const [frontmatterOpen, setFrontmatterOpen] = React.useState(false)
  const [tipsOpen, setTipsOpen] = React.useState(true)
  const [workspaceMode, setWorkspaceMode] = React.useState<'artifact' | 'bundle'>('artifact')
  const [publishCapability, setPublishCapability] = React.useState<DepotPublishCapability | null>(null)
  const [publishing, setPublishing] = React.useState(false)
  const publishingRef = React.useRef(false)
  const [linkingOwner, setLinkingOwner] = React.useState(false)
  const linkingOwnerRef = React.useRef(false)
  const [publishMessage, setPublishMessage] = React.useState('')
  const [publishError, setPublishError] = React.useState('')
  const sessionEpoch = React.useSyncExternalStore(subscribeToBrowserSession, getBrowserSessionEpoch, () => 0)
  React.useEffect(() => {
    const controller = new AbortController()
    setPublishCapability(null)
    setPublishMessage('')
    setPublishError('')
    void depotPublishCapability(controller.signal).then((capability) => {
      if (!controller.signal.aborted) setPublishCapability(capability)
    }).catch((error: unknown) => {
      if (!controller.signal.aborted) setPublishError(error instanceof Error ? error.message : 'Could not check publishing access.')
    })
    return () => controller.abort()
  }, [sessionEpoch])

  const source = React.useMemo(() => composeArtifactSource(kind, metadata, content), [content, kind, metadata])
  const issues = React.useMemo(() => validateArtifactDraft(kind, metadata, content), [content, kind, metadata])
  const authoringValidation = React.useMemo(
    () => kind === 'Skill' ? skillAuthoringSummary(metadata, content) : artifactValidationSummary(issues),
    [content, issues, kind, metadata],
  )
  const errors = issues.filter((entry) => entry.severity === 'error')
  const canPublish = publishCapability?.available === true && kind === 'Skill' && workspaceMode === 'artifact' && errors.length === 0
  const ownerLinkPending = publishCapability?.reason === 'owner_link_approval_pending'
  const confirmOwnerLink = async () => {
    if (!ownerLinkPending || linkingOwnerRef.current) return
    linkingOwnerRef.current = true
    setLinkingOwner(true)
    setPublishError('')
    const linkingSessionEpoch = getBrowserSessionEpoch()
    try {
      await consumeOwnerLinkApproval()
      if (linkingSessionEpoch !== getBrowserSessionEpoch()) return
      const capability = await depotPublishCapability()
      if (linkingSessionEpoch !== getBrowserSessionEpoch()) return
      setPublishCapability(capability)
      toast.success('Your account is linked to the existing team owner')
    } catch (error) {
      if (linkingSessionEpoch === getBrowserSessionEpoch()) {
        setPublishError(`${error instanceof Error ? error.message : 'Could not confirm the owner link.'} Refresh this page to check the current link before retrying.`)
      }
    } finally {
      linkingOwnerRef.current = false
      setLinkingOwner(false)
    }
  }
  const publish = async () => {
    if (!canPublish || publishingRef.current) return
    publishingRef.current = true
    setPublishing(true)
    setPublishError('')
    setPublishMessage('')
    const submittingSessionEpoch = getBrowserSessionEpoch()
    try {
      const receipt = await publishDepotSkill(metadata.name, source)
      if (submittingSessionEpoch !== getBrowserSessionEpoch()) return
      const message = `Publishing accepted. Job ${receipt.jobId}: ${receipt.status}. Check Depot jobs for the final result.`
      setPublishMessage(message)
      toast.success('Skill submitted to Team Depot')
    } catch (error) {
      if (submittingSessionEpoch === getBrowserSessionEpoch()) {
        setPublishError(error instanceof Error ? error.message : 'Publishing failed. Check Depot jobs before retrying.')
      }
    } finally {
      publishingRef.current = false
      setPublishing(false)
    }
  }

  const updateMetadata = (field: Exclude<keyof ArtifactMetadata, 'tags'>) => (value: string) => setMetadata((current) => ({ ...current, [field]: value }))
  const addTag = () => {
    const tag = tagInput.trim().replace(/^#/, '').toLowerCase()
    if (!tag) return
    setMetadata(current => current.tags.includes(tag) ? current : { ...current, tags: [...current.tags, tag] })
    setTagInput('')
  }
  const removeTag = (tag: string) => setMetadata(current => ({ ...current, tags: current.tags.filter(item => item !== tag) }))
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
    setTagInput('')
  }

  const headerActions = <div className="flex translate-x-[2px] flex-wrap items-center gap-[7px]">
        <Button asChild variant="outline" className="h-9 gap-[7px] rounded-[10px] px-[13px] text-[12.5px] font-[650]" data-visible-label="1"><a href="/administration/" title="Depot operations — Administration"><ShieldCheck className="size-[13px]" />Depot Operations</a></Button>
        <DropdownMenu>
          <Tooltip><TooltipTrigger asChild><DropdownMenuTrigger asChild><Button size="icon" variant="outline" className="size-9 rounded-[10px]" aria-label="More artifact actions"><MoreHorizontal className="size-[15px]" /></Button></DropdownMenuTrigger></TooltipTrigger><TooltipContent sideOffset={7}>More actions</TooltipContent></Tooltip>
          <DropdownMenuContent align="end" className="min-w-52 border-aurora-border-strong bg-aurora-panel-strong">
            <DropdownMenuLabel>Artifact actions</DropdownMenuLabel><DropdownMenuSeparator />
            <DropdownMenuItem onSelect={() => void copySource()}><Clipboard />Copy complete source</DropdownMenuItem>
            <DropdownMenuItem onSelect={() => setFrontmatterOpen(true)}><Settings2 />Edit frontmatter</DropdownMenuItem>
            <DropdownMenuItem onSelect={() => setTipsOpen(open => !open)}><CircleHelp />{tipsOpen ? 'Hide writing tips' : 'Show writing tips'}</DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={reset}><RotateCcw />Restore starter</DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        <Button size="sm" aria-label="Publish skill" className="h-9 w-[100px] rounded-[10px] px-4 text-[13px] font-bold" disabled={!canPublish || publishing} onClick={() => void publish()}>{publishing ? 'Submitting…' : 'Publish'}</Button>
      </div>

  const bundleGroups = [['Agent',['rust-reviewer']],['Command',['/ship','/scope-audit']],['Skill',['repo-triage','changelog-writer']],['Hook',['pre-commit-guard']],['MCP',['labby','axon']],['Prompt',[]]] as const
  return <>
    {publishError || ownerLinkPending || publishMessage ? (
      <div className="px-space-5 py-space-2 text-sm text-aurora-text-muted" aria-live="polite">
        {publishError ? <p role="alert" className="text-aurora-error">{publishError}</p> : ownerLinkPending ? <div className="flex flex-wrap items-center gap-space-4"><p>An operator approved linking this signed-in account to the existing team owner. Existing owner logins will be preserved.</p><Button variant="outline" size="sm" disabled={linkingOwner} onClick={() => void confirmOwnerLink()}>{linkingOwner ? 'Confirming owner link…' : 'Confirm owner link'}</Button></div> : <p>{publishMessage} <a className="underline" href="/administration/">Open Depot operations</a></p>}
      </div>
    ) : null}
    <AppHeader icon={<PencilLine className="size-[18px] text-aurora-accent-pink" />} breadcrumbs={[{ label: 'Create' }]} />
    <div className={cn(AURORA_PAGE_SHELL, 'flex-1')}><div className={cn(AURORA_PAGE_FRAME, 'min-h-[calc(100vh-5.5rem)] gap-3.5')}>
      <ConsoleHero
        variant="authoring"
        eyebrow="Depot · Authoring"
        title="Create"
        icon={<PencilLine className="size-[21px] text-aurora-accent-pink" />}
        description="Author an artifact in one document. Validation updates as you type. Skill publishing is available when your session permits it."
        actions={headerActions}
        pulse={{ color: 'var(--aurora-success)', label: 'local draft' }}
        stats={[
          { label: 'Kind', value: workspaceMode === 'artifact' ? kind : 'Bundle', suffix: workspaceMode === 'artifact' ? 'artifact' : 'preview' },
          { label: 'Validation', value: authoringValidation.passing < authoringValidation.total ? 'Needs work' : 'Passing', suffix: `${authoringValidation.passing}/${authoringValidation.total} checks`, tone: authoringValidation.passing < authoringValidation.total ? 'var(--aurora-warn)' : 'var(--aurora-success)' },
          { label: 'Formats', value: workspaceMode === 'artifact' ? '1' : '0', suffix: workspaceMode === 'artifact' ? `${artifactLanguage(kind) === 'markdown' ? 'Markdown' : artifactLanguage(kind) === 'bash' ? 'Shell' : 'JSON'} source` : 'bundle export unavailable', tone: 'var(--aurora-accent-strong)' },
        ]}
      ><ArtifactWorkspaceSwitch value={workspaceMode} onChange={setWorkspaceMode} tabs /></ConsoleHero>
      <div className="w-full min-w-0">
        <div aria-label="Creation toolbar" hidden className="hidden">
          <DropdownMenu><DropdownMenuTrigger data-visible-label="1" className={cn('inline-flex h-[30px] shrink-0 items-center gap-[7px] rounded-full border px-3 text-[11.5px] font-[650] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary', issues.length ? 'border-aurora-warn text-aurora-warn' : 'border-aurora-success text-aurora-success')}>{issues.length ? <CircleAlert className="size-[13px]" /> : <CircleCheck className="size-[13px]" />}{issues.length ? `${issues.length} issue${issues.length === 1 ? '' : 's'}` : 'All checks pass'}</DropdownMenuTrigger><DropdownMenuContent align="end" className="w-80 max-w-[calc(100vw-32px)]"><DropdownMenuLabel>Draft validation</DropdownMenuLabel>{issues.length ? issues.map((issue, index) => <DropdownMenuItem key={`${issue.field}-${index}`} className="items-start whitespace-normal" onSelect={() => { setWorkspaceMode('artifact'); setTipsOpen(true) }}><CircleAlert className="size-[13px] shrink-0" /><span>{issue.message}</span></DropdownMenuItem>) : <div className="px-2 py-2 text-[11.5px] text-aurora-text-muted">All checks pass.</div>}</DropdownMenuContent></DropdownMenu>
          {workspaceMode === 'artifact' ? <button type="button" aria-expanded={tipsOpen} aria-controls="artifact-writing-tips" title="Writing tips — best practices for this kind" data-visible-label="1" onClick={() => setTipsOpen(open => !open)} className={cn('inline-flex h-[30px] shrink-0 items-center gap-1.5 rounded-full border px-3 text-[11.5px] font-[650] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary', tipsOpen ? 'border-aurora-accent-primary/35 bg-aurora-selected-bg text-aurora-accent-strong' : 'border-aurora-border-default bg-aurora-control-surface text-aurora-text-muted')}><CircleHelp className="size-[13px]" />Tips</button> : null}
        </div>
        {workspaceMode==='artifact'?<div className={cn('grid items-start gap-3', tipsOpen && 'lg:grid-cols-[minmax(0,1fr)_272px]')}>
          <section className="min-w-0 overflow-hidden rounded-aurora-3 border border-aurora-border-strong bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] shadow-aurora-strong">
            <div aria-label="Document controls" className="flex flex-wrap items-center gap-2 border-b border-aurora-border-default bg-aurora-control-surface py-[9px] pl-3.5 pr-3">
              <ArtifactKindPicker value={kind} onChange={setKind} />
              <span className="ml-auto text-[11px] text-aurora-text-muted" title="This draft exists only in this browser view and has not been saved.">Unsaved draft</span>
            <div role="group" aria-label="Document view" className="flex w-fit shrink-0 gap-0.5 rounded-lg border border-aurora-border-default bg-aurora-page-bg p-0.5">
              {(['source', 'preview'] as const).map(mode => <button key={mode} type="button" aria-pressed={documentView === mode} onClick={() => setDocumentView(mode)} className={cn('h-[22px] rounded px-2.5 text-[11px] font-semibold focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary', documentView === mode ? 'bg-aurora-selected-bg text-aurora-accent-strong' : 'text-aurora-text-muted hover:text-aurora-text-primary')}>{mode === 'source' ? 'Source' : 'Preview'}</button>)}
            </div>
            </div>
            <div className="px-5 pb-[22px] pt-[26px] sm:px-[34px]">
            <div className="grid grid-cols-[6px_minmax(0,1fr)] items-start gap-x-3">
            <ArtifactFieldIndicator field="name" issues={issues} className="mt-[18px]" />
            <input aria-label="Artifact name" placeholder="untitled-artifact" spellCheck={false} value={metadata.name} onChange={(event)=>updateMetadata('name')(event.target.value)} className="-mx-1.5 h-12 w-[calc(100%+12px)] rounded-t-lg border border-transparent border-b-aurora-border-strong bg-transparent px-1.5 py-1 font-display text-[28px] font-extrabold tracking-[-.015em] text-aurora-text-primary outline-none transition-colors [border-bottom-style:dotted] hover:bg-aurora-hover-bg focus:rounded-lg focus:border-aurora-accent-primary focus:bg-aurora-control-surface focus:shadow-[var(--aurora-focus-ring-strong)] focus:[border-bottom-style:solid]"/>
            <ArtifactFieldIndicator field="description" issues={issues} className="mt-[17px]" />
            <ArtifactDescriptionField value={metadata.description} onChange={updateMetadata('description')} />
            </div>
            <div className="mt-3 grid grid-cols-[6px_minmax(0,1fr)] items-start gap-x-3">
              <span aria-hidden="true" />
              <div className="flex h-6 min-w-0 items-center gap-1.5">
                {metadata.tags.map(tag => <span key={tag} className="inline-flex h-6 shrink-0 items-center gap-[5px] rounded-full border border-aurora-accent-primary/25 bg-aurora-accent-primary/8 pl-[10px] pr-1.5 text-[11.5px] text-aurora-accent-strong"><span>#{tag}</span><button type="button" aria-label={`Remove tag ${tag}`} onClick={() => removeTag(tag)} className="grid size-[14px] place-items-center rounded-full text-aurora-text-muted hover:text-aurora-text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary"><X className="size-2.5" /></button></span>)}
                <input aria-label="Add a tag" value={tagInput} onChange={event => setTagInput(event.target.value)} onKeyDown={event => { if (event.key === 'Enter' || event.key === ',') { event.preventDefault(); addTag() } }} onBlur={addTag} placeholder="add tag…" className="h-[18px] min-w-[80px] flex-1 bg-transparent px-0 py-0.5 text-[11.5px] text-aurora-text-primary outline-none placeholder:text-aurora-text-muted" />
              </div>
            </div>
            {frontmatterOpen ? <fieldset className="mb-6 grid gap-3 rounded-lg border border-aurora-border-default bg-aurora-control-surface p-4">
              <legend className="px-1 text-xs font-semibold text-aurora-text-muted">Frontmatter fields</legend>
              {(['license', 'compatibility', 'allowedTools'] as const).map(field => <label key={field} className="grid gap-1 text-xs text-aurora-text-muted">{field === 'allowedTools' ? 'Allowed tools' : field === 'license' ? 'License' : 'Compatibility'}<input value={metadata[field]} onChange={event => updateMetadata(field)(event.target.value)} className="h-8 rounded border border-aurora-border-default bg-aurora-page-bg px-2 text-aurora-text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary" /></label>)}
            </fieldset> : null}
            </div>
            <div data-artifact-source="1" hidden={documentView !== 'source'} className="border-t border-aurora-border-subtle [&[hidden]]:hidden">
            {artifactLanguage(kind) === 'markdown' ? <ArtifactFormattingToolbar indicator={<ArtifactFieldIndicator field="content" issues={issues} className="mr-2" />} onSection={section => append(`## ${section}`)} onFormat={format => {
              const editor = contentRef.current
              if (!editor) return
              const result = formatArtifactSelection(content, editor.selectionStart, editor.selectionEnd, format)
              setContent(result.content)
              requestAnimationFrame(() => { editor.focus(); editor.setSelectionRange(result.cursor, result.cursor) })
            }} /> : null}
            <div className="relative min-w-0 bg-aurora-control-surface pl-11">
              <div aria-hidden="true" className="pointer-events-none absolute inset-y-0 left-0 w-11 select-none overflow-hidden border-r border-aurora-border-subtle bg-aurora-page-bg text-right font-mono text-[12.5px] leading-[1.7] tabular-nums text-aurora-text-muted"><div ref={gutterRef} className="whitespace-pre py-3.5 pr-2.5 opacity-70">{Array.from({ length: content.split('\n').length }, (_, index) => index + 1).join('\n')}</div></div>
              <textarea ref={contentRef} aria-label="Artifact content" wrap="off" spellCheck={false} placeholder="Write the instructions…" value={content} onScroll={event => { if (gutterRef.current) gutterRef.current.style.transform = `translateY(-${event.currentTarget.scrollTop}px)` }} onChange={(event)=>setContent(event.target.value)} className="aurora-scrollbar block min-h-[360px] w-full resize-y overflow-x-auto whitespace-pre bg-transparent pb-[18px] pl-3.5 pr-[18px] pt-3.5 font-mono text-[12.5px] leading-[1.7] text-aurora-text-primary outline-none [tab-size:2]"/>
            </div>
            </div>
            {documentView === 'preview' ? <div aria-label="Artifact preview" title="Double-click to edit source" onDoubleClick={() => { setDocumentView('source'); requestAnimationFrame(() => contentRef.current?.focus()) }} className="min-h-[360px] cursor-text border-t border-aurora-border-subtle px-5 pb-[26px] pt-[22px] sm:px-[34px]">{artifactLanguage(kind) === 'markdown' ? <SafeMarkdown text={content} className="max-w-[78ch] text-sm leading-[1.65]" /> : <pre className="whitespace-pre-wrap break-words font-mono text-sm leading-6">{content}</pre>}</div> : null}
            <div aria-label="Artifact document status" className="flex items-center gap-2.5 border-t border-aurora-border-subtle bg-aurora-control-surface px-4 py-1.5 text-[10.5px] tabular-nums text-aurora-text-muted"><span className="min-w-0 flex-1 truncate">{artifactPath(kind, metadata.name)}</span><span className="shrink-0" title="Approximate tokens (characters ÷ 4)">~{Math.ceil(content.length / 4)} tokens · {content.trim() ? content.trim().split(/\s+/).length : 0} words · {content.length} chars</span></div>
          </section>
          <div id="artifact-writing-tips" hidden={!tipsOpen} className="flex flex-col [&[hidden]]:hidden [&>section]:order-2">
          <ArtifactFrontmatterPreview kind={kind} metadata={metadata} />
          <ArtifactWritingExample kind={kind} />
          <ArtifactValidationPanel kind={kind} metadata={metadata} content={content} issues={issues} onField={field => {
            if (field === 'content') {
              setDocumentView('source')
              requestAnimationFrame(() => contentRef.current?.focus())
              return
            }
            const labels = { name: 'Artifact name', description: 'Artifact description', tags: 'Add a tag', content: 'Artifact content' }
            if (field in labels) document.querySelector<HTMLElement>(`[aria-label="${labels[field as keyof typeof labels]}"]`)?.focus()
            else setFrontmatterOpen(true)
          }} />
          </div>
        </div>:<section className="rounded-aurora-3 border border-aurora-border-strong bg-aurora-panel-medium p-8 shadow-aurora-panel"><h1 className="text-3xl font-bold">{metadata.name}</h1><div className="mt-4 flex flex-wrap gap-2 text-xs text-aurora-text-muted"><span className="font-bold uppercase tracking-wider">Potential targets</span>{['Loadout','Claude plugin.json','marketplace.json','gemini-extension.json','Agent Plugins','ARD ai-catalog.json'].map((item)=><Badge key={item} variant="outline">{item}</Badge>)}</div><div className="mt-7 grid gap-3 md:grid-cols-2 lg:grid-cols-3">{bundleGroups.map(([group,items])=><div key={group} className="min-h-36 rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-low p-3"><div className="flex justify-between text-xs font-bold uppercase tracking-wider text-aurora-accent-primary"><span>{group}</span><span>{items.length}</span></div><div className="mt-3 space-y-2">{items.map((item)=><div key={item} className="rounded-aurora-1 bg-aurora-control-surface px-3 py-2 text-sm"><span>{item}</span></div>)}</div></div>)}</div></section>}
      </div>
    </div></div>
  </>
}
