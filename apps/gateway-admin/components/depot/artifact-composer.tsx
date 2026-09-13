'use client'

import * as React from 'react'
import {
  Clipboard, Download, ShieldCheck, Upload,
  MoreHorizontal, RotateCcw, Settings2, CircleHelp, PencilLine, X,
} from 'lucide-react'

import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
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
  ARTIFACT_KINDS, type ArtifactKind, type ArtifactMetadata, validateArtifactDraft,
} from '@/lib/editor/artifact-standards'
import { cn } from '@/lib/utils'
import { toast } from 'sonner'
import { consumeOwnerLinkApproval, depotPublishCapability, publishDepotSkill, type DepotPublishCapability } from '@/lib/api/depot-client'
import { getBrowserSessionContextIdentity, getBrowserSessionEpoch, getBrowserSessionState, subscribeToBrowserSession } from '@/lib/auth/session-store'
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

type WorkspaceMode = 'artifact' | 'bundle'
type DraftStatus = 'draft' | 'unsaved' | 'saved' | 'credential' | 'error' | 'session'
type StoredCreateDraft = {
  version: 1
  kind: ArtifactKind
  metadata: ArtifactMetadata
  content: string
  workspaceMode: WorkspaceMode
  bundleEntries: Record<string, string>
}
const CREATE_DRAFT_PREFIX = 'labby:create-draft:v1:'
const CREATE_DRAFT_SAVE_DELAY_MS = 120

export function createDraftStorageKey(contextIdentity: string) {
  return `${CREATE_DRAFT_PREFIX}${encodeURIComponent(contextIdentity)}`
}

function safeCredentialReference(value: string) {
  return /^(?:\$\{[A-Z][A-Z0-9_]*\}|\$[A-Z][A-Z0-9_]*|\{\{[^{}]+\}\}|<[^<>]+>)$/.test(value.trim())
}

function credentialInValue(value: unknown, key = ''): boolean {
  if (typeof value === 'string') {
    if (/\bBearer\s+(?!\$\{|\$[A-Z_]|\{\{|<)[A-Za-z0-9._~+/=-]{8,}/i.test(value)) return true
    if (/\b(?:api[_-]?key|access[_-]?token|authorization|cookie|password|private[_-]?key|secret|token)\b["']?\s*[:=]\s*["']?(?!\$\{|\$[A-Z_]|\{\{|<)[^\s"',}\]]{4,}/i.test(value)) return true
    return Boolean(key && /(?:api[_-]?key|access[_-]?token|authorization|cookie|password|private[_-]?key|secret|token)/i.test(key) && value.trim() && !safeCredentialReference(value))
  }
  if (Array.isArray(value)) return value.some(item => credentialInValue(item, key))
  if (!value || typeof value !== 'object') return false
  return Object.entries(value).some(([childKey, child]) => credentialInValue(child, childKey))
}

export function draftMayContainCredential(value: string) {
  try { return credentialInValue(JSON.parse(value)) } catch { return credentialInValue(value) }
}

function bundleSource(name: string, entries: Record<string, string>) {
  return JSON.stringify({
    name,
    artifacts: Object.fromEntries(Object.entries(entries)
      .map(([group, text]) => [group, text.split('\n').map(value => value.trim()).filter(Boolean)] as const)
      .filter(([, values]) => values.length > 0)),
  }, null, 2)
}

export function createArtifactDownload(mode: WorkspaceMode, kind: ArtifactKind, name: string, source: string, entries: Record<string, string>) {
  const safeName = name.trim().replace(/[^a-z0-9._-]+/gi, '-') || 'untitled'
  if (mode === 'bundle') return { filename: `${safeName}-bundle.json`, type: 'application/json;charset=utf-8', content: bundleSource(name, entries) }
  const language = artifactLanguage(kind)
  const filename = kind === 'Skill' ? `${safeName}-SKILL.md` : `${safeName}.${language === 'markdown' ? 'md' : language === 'bash' ? 'sh' : 'json'}`
  return { filename, type: language === 'markdown' ? 'text/markdown;charset=utf-8' : language === 'json' ? 'application/json;charset=utf-8' : 'text/plain;charset=utf-8', content: source }
}

function parseStoredDraft(raw: string): StoredCreateDraft | null {
  if (new TextEncoder().encode(raw).length > 1_000_000) return null
  try {
    const value = JSON.parse(raw) as Partial<StoredCreateDraft>
    const metadata = value.metadata
    if (value.version !== 1 || !ARTIFACT_KINDS.includes(value.kind as ArtifactKind) || (value.workspaceMode !== 'artifact' && value.workspaceMode !== 'bundle') || typeof value.content !== 'string' || !metadata || typeof metadata !== 'object') return null
    if (typeof metadata.name !== 'string' || typeof metadata.description !== 'string' || !Array.isArray(metadata.tags) || !metadata.tags.every(tag => typeof tag === 'string') || typeof metadata.license !== 'string' || typeof metadata.compatibility !== 'string' || typeof metadata.allowedTools !== 'string') return null
    if (!value.bundleEntries || typeof value.bundleEntries !== 'object' || Array.isArray(value.bundleEntries) || !Object.values(value.bundleEntries).every(entry => typeof entry === 'string')) return null
    return value as StoredCreateDraft
  } catch { return null }
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
  const [workspaceMode, setWorkspaceMode] = React.useState<WorkspaceMode>('artifact')
  const [bundleEntries, setBundleEntries] = React.useState<Record<string, string>>({})
  const [draftStatus, setDraftStatus] = React.useState<DraftStatus>('draft')
  const [draftLoaded, setDraftLoaded] = React.useState(false)
  const [publishCapability, setPublishCapability] = React.useState<DepotPublishCapability | null>(null)
  const [publishing, setPublishing] = React.useState(false)
  const publishingRef = React.useRef(false)
  const [linkingOwner, setLinkingOwner] = React.useState(false)
  const linkingOwnerRef = React.useRef(false)
  const [publishMessage, setPublishMessage] = React.useState('')
  const [publishError, setPublishError] = React.useState('')
  const sessionEpoch = React.useSyncExternalStore(subscribeToBrowserSession, getBrowserSessionEpoch, () => 0)
  const sessionState = getBrowserSessionState()
  const draftStorageKey = sessionState.status === 'authenticated' ? createDraftStorageKey(getBrowserSessionContextIdentity()) : null
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

  React.useEffect(() => {
    setDraftLoaded(false)
    if (!draftStorageKey) {
      setDraftStatus('session')
      return
    }
    try {
      const raw = window.localStorage.getItem(draftStorageKey)
      const saved = raw ? parseStoredDraft(raw) : null
      if (saved) {
        setKind(saved.kind)
        setMetadata(saved.metadata)
        setContent(saved.content)
        setWorkspaceMode(saved.workspaceMode)
        setBundleEntries(saved.bundleEntries)
        setDraftStatus('saved')
      } else {
        setKind('Skill')
        setMetadata(STARTER_METADATA)
        setContent(STARTER_BODY)
        setWorkspaceMode('artifact')
        setBundleEntries({})
        setDraftStatus('draft')
      }
    } catch { setDraftStatus('error') }
    setDraftLoaded(true)
  }, [draftStorageKey, sessionEpoch])

  const source = React.useMemo(() => composeArtifactSource(kind, metadata, content), [content, kind, metadata])
  const storedDraft = React.useMemo<StoredCreateDraft>(() => ({ version: 1, kind, metadata, content, workspaceMode, bundleEntries }), [bundleEntries, content, kind, metadata, workspaceMode])
  React.useEffect(() => {
    if (!draftLoaded || draftStatus !== 'unsaved' || !draftStorageKey) return
    const timer = window.setTimeout(() => {
      const serialized = JSON.stringify(storedDraft)
      if (draftMayContainCredential(serialized)) {
        try { window.localStorage.removeItem(draftStorageKey) } catch { /* The status below still reports that nothing was saved. */ }
        setDraftStatus('credential')
        return
      }
      try {
        window.localStorage.setItem(draftStorageKey, serialized)
        setDraftStatus('saved')
      } catch { setDraftStatus('error') }
    }, CREATE_DRAFT_SAVE_DELAY_MS)
    return () => window.clearTimeout(timer)
  }, [draftLoaded, draftStatus, draftStorageKey, storedDraft])
  const issues = React.useMemo(() => validateArtifactDraft(kind, metadata, content), [content, kind, metadata])
  const authoringValidation = React.useMemo(
    () => kind === 'Skill' ? skillAuthoringSummary(metadata, content, issues) : artifactValidationSummary(issues),
    [content, issues, kind, metadata],
  )
  const errors = issues.filter((entry) => entry.severity === 'error')
  const unavailableReason = publishCapability?.reason === 'project_session_required'
    ? 'Open an authenticated team project session to publish. Your current sign-in has no project publishing authority.'
    : publishCapability?.reason
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

  const markDraftDirty = () => setDraftStatus(draftStorageKey ? 'unsaved' : 'session')
  const updateMetadata = (field: Exclude<keyof ArtifactMetadata, 'tags'>) => (value: string) => { markDraftDirty(); setMetadata((current) => ({ ...current, [field]: value })) }
  const addTag = () => {
    const tag = tagInput.trim().replace(/^#/, '').toLowerCase()
    if (!tag) return
    setMetadata(current => current.tags.includes(tag) ? current : { ...current, tags: [...current.tags, tag] })
    markDraftDirty()
    setTagInput('')
  }
  const removeTag = (tag: string) => { markDraftDirty(); setMetadata(current => ({ ...current, tags: current.tags.filter(item => item !== tag) })) }
  const copySource = React.useCallback(async () => {
    try {
      await navigator.clipboard.writeText(workspaceMode === 'artifact' ? source : bundleSource(metadata.name, bundleEntries))
      toast.success('Complete source copied')
    } catch {
      toast.error('Could not copy complete source')
    }
  }, [source, workspaceMode, metadata.name, bundleEntries])
  const downloadSource = React.useCallback(() => {
    const download = createArtifactDownload(workspaceMode, kind, metadata.name, source, bundleEntries)
    const url = URL.createObjectURL(new Blob([download.content], { type: download.type }))
    const anchor = document.createElement('a')
    anchor.href = url
    anchor.download = download.filename
    anchor.click()
    URL.revokeObjectURL(url)
  }, [bundleEntries, kind, metadata.name, source, workspaceMode])
  const append = (snippet: string) => { markDraftDirty(); setContent((current) => `${current.replace(/\s+$/, '')}\n\n${snippet}\n`) }
  const reset = () => {
    setKind('Skill')
    setMetadata(STARTER_METADATA)
    setContent(STARTER_BODY)
    setTagInput('')
    setBundleEntries({})
    setDraftStatus(draftStorageKey ? 'draft' : 'session')
    if (draftStorageKey) {
      try { window.localStorage.removeItem(draftStorageKey) } catch { setDraftStatus('error') }
    }
  }

  const draftStatusText = draftStatus === 'saved' ? 'Saved locally'
    : draftStatus === 'unsaved' ? 'Unsaved changes'
      : draftStatus === 'credential' ? 'Not saved — credential detected'
        : draftStatus === 'error' ? 'Draft could not be saved on this device'
          : draftStatus === 'session' ? 'Not saved — authenticated workspace required'
            : 'Local draft'

  const headerActions = <div className="flex flex-wrap items-center gap-[7px]">
        <Button asChild variant="outline" className="h-9 gap-[7px] rounded-[10px] px-[13px] text-[12.5px] font-[650]" data-visible-label="1"><a href="/administration/" title="Depot operations — Administration"><ShieldCheck className="size-[13px]" />Depot Operations</a></Button>
        <DropdownMenu>
          <Tooltip><TooltipTrigger asChild><DropdownMenuTrigger asChild><Button size="icon" variant="outline" className="size-9 rounded-[10px]" aria-label="More artifact actions"><MoreHorizontal className="size-[15px]" /></Button></DropdownMenuTrigger></TooltipTrigger><TooltipContent sideOffset={7}>More actions</TooltipContent></Tooltip>
          <DropdownMenuContent align="end" className="min-w-52 border-aurora-border-strong bg-aurora-panel-strong">
            <DropdownMenuLabel>Artifact actions</DropdownMenuLabel><DropdownMenuSeparator />
            <DropdownMenuItem onSelect={() => void copySource()}><Clipboard />Copy complete source</DropdownMenuItem>
            <DropdownMenuItem onSelect={downloadSource}><Download />Download {workspaceMode === 'bundle' ? 'bundle JSON' : `${kind.toLowerCase()} file`}</DropdownMenuItem>
            <DropdownMenuItem onSelect={() => setFrontmatterOpen(true)}><Settings2 />Edit frontmatter</DropdownMenuItem>
            <DropdownMenuItem onSelect={() => setTipsOpen(open => !open)}><CircleHelp />{tipsOpen ? 'Hide writing tips' : 'Show writing tips'}</DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={reset}><RotateCcw />Restore starter</DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        <Button data-visible-label size="sm" aria-label="Publish skill" className="h-9 w-[100px] gap-[7px] rounded-[10px] border border-[color-mix(in_srgb,var(--aurora-accent-pink)_70%,#0b2233)] bg-aurora-accent-pink px-4 text-[13px] font-bold text-[#2a0f18] shadow-[0_0_0_1px_color-mix(in_srgb,var(--aurora-accent-pink)_30%,transparent),inset_0_1px_0_rgba(255,255,255,0.25)] hover:bg-[color-mix(in_srgb,#fff_8%,var(--aurora-accent-pink))] hover:text-[#2a0f18]" disabled={!canPublish || publishing} onClick={() => void publish()}><Upload className="size-[13px]" />{publishing ? 'Submitting…' : 'Publish'}</Button>
      </div>

  const bundleGroups = ['Agent', 'Command', 'Skill', 'Hook', 'MCP', 'Prompt'] as const
  return <>
    {publishError || ownerLinkPending || publishMessage || unavailableReason ? (
      <div className="px-space-5 py-space-2 text-sm text-aurora-text-muted" aria-live="polite">
        {publishError ? <p role="alert" className="text-aurora-error">{publishError}</p> : ownerLinkPending ? <div className="flex flex-wrap items-center gap-space-4"><p>An operator approved linking this signed-in account to the existing team owner. Existing owner logins will be preserved.</p><Button variant="outline" size="sm" disabled={linkingOwner} onClick={() => void confirmOwnerLink()}>{linkingOwner ? 'Confirming owner link…' : 'Confirm owner link'}</Button></div> : publishMessage ? <p>{publishMessage} <a className="underline" href="/administration/">Open Depot operations</a></p> : <p>{unavailableReason}</p>}
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
          { label: 'Validation', value: workspaceMode === 'bundle' ? '—' : authoringValidation.passing < authoringValidation.total ? 'Needs work' : 'Passing', suffix: workspaceMode === 'bundle' ? 'bundle validation unavailable' : `${authoringValidation.passing}/${authoringValidation.total} checks`, tone: authoringValidation.passing < authoringValidation.total ? 'var(--aurora-warn)' : 'var(--aurora-success)' },
          { label: 'Formats', value: workspaceMode === 'artifact' ? '1' : '0', suffix: workspaceMode === 'artifact' ? `${artifactLanguage(kind) === 'markdown' ? 'Markdown' : artifactLanguage(kind) === 'bash' ? 'Shell' : 'JSON'} source` : 'bundle export unavailable', tone: 'var(--aurora-accent-strong)' },
        ]}
      ><ArtifactWorkspaceSwitch value={workspaceMode} onChange={value => { markDraftDirty(); setWorkspaceMode(value) }} tabs /></ConsoleHero>
      <div className="w-full min-w-0">
        {workspaceMode==='artifact'?<div className={cn('grid items-start gap-3', tipsOpen && 'lg:grid-cols-[minmax(0,1fr)_272px]')}>
          <section className="min-w-0 overflow-hidden rounded-aurora-3 border border-[color-mix(in_srgb,var(--aurora-border-default)_45%,var(--aurora-page-bg))] bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] shadow-aurora-strong">
            <div aria-label="Document controls" className="flex flex-wrap items-center gap-2 border-b border-aurora-border-default/55 bg-[var(--gw0-0_38)] py-[9px] pl-3.5 pr-3">
              <ArtifactKindPicker value={kind} onChange={value => { markDraftDirty(); setKind(value) }} />
              <span aria-label="Draft save status" aria-live="polite" className={cn('ml-auto text-[11px]', draftStatus === 'error' || draftStatus === 'credential' ? 'text-aurora-warn' : 'text-aurora-text-muted')} title="Drafts are stored only in this browser and only for the current authenticated authority workspace.">{draftStatusText}</span>
            <div role="group" aria-label="Document view" className="flex w-fit shrink-0 gap-0.5 rounded-lg border border-aurora-border-default/55 bg-[var(--gw0-0_40)] p-0.5">
              {(['source', 'preview'] as const).map(mode => <button key={mode} type="button" aria-pressed={documentView === mode} onClick={() => setDocumentView(mode)} className={cn('h-[22px] rounded-md px-2.5 text-[10.5px] font-[650] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary', documentView === mode ? 'bg-aurora-selected-bg text-aurora-accent-strong' : 'text-aurora-text-muted hover:text-aurora-text-primary')}>{mode === 'source' ? 'Source' : 'Preview'}</button>)}
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
              <ArtifactFieldIndicator field="tags" issues={issues} className="mt-2" />
              <div className="flex h-6 min-w-0 items-center gap-1.5">
                {metadata.tags.map(tag => <span key={tag} className="inline-flex h-6 shrink-0 items-center gap-[5px] rounded-full border border-aurora-accent-primary/25 bg-aurora-accent-primary/8 pl-[10px] pr-1.5 text-[11.5px] text-aurora-accent-strong"><span>#{tag}</span><button type="button" aria-label={`Remove tag ${tag}`} onClick={() => removeTag(tag)} className="grid size-[14px] place-items-center rounded-full text-aurora-text-muted hover:text-aurora-text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary"><X className="size-2.5" /></button></span>)}
                <input aria-label="Add a tag" value={tagInput} onChange={event => setTagInput(event.target.value)} onKeyDown={event => {
                  if ((event.key === 'Enter' || event.key === ',' || event.key === ' ') && tagInput.trim()) { event.preventDefault(); addTag() }
                  else if (event.key === 'Backspace' && !tagInput && metadata.tags.length) removeTag(metadata.tags.at(-1)!)
                }} onBlur={addTag} placeholder="add tag…" className="h-[18px] min-w-[80px] flex-1 bg-transparent px-0 py-0.5 text-[11.5px] text-aurora-text-primary outline-none placeholder:text-aurora-text-muted" />
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
              markDraftDirty()
              setContent(result.content)
              requestAnimationFrame(() => { editor.focus(); editor.setSelectionRange(result.cursor, result.cursor) })
            }} /> : null}
            <div className="relative min-w-0 bg-aurora-control-surface pl-11">
              <div aria-hidden="true" className="pointer-events-none absolute inset-y-0 left-0 w-11 select-none overflow-hidden border-r border-aurora-border-subtle bg-aurora-page-bg text-right font-mono text-[12.5px] leading-[1.7] tabular-nums text-aurora-text-muted"><div ref={gutterRef} className="whitespace-pre py-3.5 pr-2.5 opacity-70">{Array.from({ length: content.split('\n').length }, (_, index) => index + 1).join('\n')}</div></div>
              <textarea ref={contentRef} aria-label="Artifact content" wrap="off" spellCheck={false} placeholder="Write the instructions…" value={content} onScroll={event => { if (gutterRef.current) gutterRef.current.style.transform = `translateY(-${event.currentTarget.scrollTop}px)` }} onKeyDown={event => {
                if (event.key !== 'Tab') return
                event.preventDefault()
                const start = event.currentTarget.selectionStart
                const end = event.currentTarget.selectionEnd
                markDraftDirty()
                setContent(`${content.slice(0, start)}  ${content.slice(end)}`)
                requestAnimationFrame(() => { contentRef.current?.focus(); contentRef.current?.setSelectionRange(start + 2, start + 2) })
              }} onChange={(event)=>{ markDraftDirty(); setContent(event.target.value) }} className="aurora-scrollbar block min-h-[360px] w-full resize-y overflow-x-auto whitespace-pre bg-transparent pb-[18px] pl-3.5 pr-[18px] pt-3.5 font-mono text-[12.5px] leading-[1.7] text-aurora-text-primary outline-none [tab-size:2]"/>
            </div>
            </div>
            {documentView === 'preview' ? <div aria-label="Artifact preview" title="Double-click to edit source" onDoubleClick={() => { setDocumentView('source'); requestAnimationFrame(() => contentRef.current?.focus()) }} className="min-h-[360px] cursor-text border-t border-aurora-border-subtle px-5 pb-[26px] pt-[22px] sm:px-[34px]">{artifactLanguage(kind) === 'markdown' ? <SafeMarkdown text={content} className="max-w-[78ch] text-sm leading-[1.65]" /> : <pre className="whitespace-pre-wrap break-words font-mono text-sm leading-6">{content}</pre>}</div> : null}
            <div aria-label="Artifact document status" className="flex items-center gap-2.5 border-t border-aurora-border-subtle bg-aurora-control-surface px-4 py-1.5 text-[10.5px] tabular-nums text-aurora-text-muted"><span className="min-w-0 flex-1 truncate">{artifactPath(kind, metadata.name)}</span><span className="shrink-0" title="Approximate tokens (characters ÷ 4)">~{Math.ceil(content.length / 4)} tokens · {content.trim() ? content.trim().split(/\s+/).length : 0} words · {content.length} chars</span></div>
          </section>
          <div id="artifact-writing-tips" hidden={!tipsOpen} className="flex flex-col gap-3 lg:sticky lg:top-3 [&[hidden]]:hidden [&>section]:order-2">
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
        </div>:<section aria-label="Bundle draft" className="rounded-aurora-3 border border-aurora-border-default/45 bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] px-8 pb-[22px] pt-7 shadow-aurora-strong">
          <input aria-label="Bundle name" value={metadata.name} onChange={event => updateMetadata('name')(event.target.value)} placeholder="untitled-bundle" className="w-full border-0 bg-transparent font-display text-[26px] font-extrabold tracking-[-.015em] outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary" />
          <div className="mt-3 flex flex-wrap items-start justify-between gap-2 text-xs text-aurora-text-muted"><p>Local bundle draft. Enter one artifact reference per line. Bundle validation, publishing, and format compilation are not available from this editor.</p><span aria-label="Draft save status" aria-live="polite" className={draftStatus === 'error' || draftStatus === 'credential' ? 'shrink-0 text-aurora-warn' : 'shrink-0'} title="Drafts are stored only in this browser and only for the current authenticated authority workspace.">{draftStatusText}</span></div>
          <div className="mt-4 flex flex-wrap gap-[7px] text-xs text-aurora-text-muted"><span className="text-[9.5px] font-bold uppercase tracking-[.13em]">Potential targets</span>{['Loadout','Claude plugin.json','marketplace.json','gemini-extension.json','Agent Plugins','ARD ai-catalog.json'].map(item => <Badge key={item} variant="outline">{item}</Badge>)}</div>
          <div className="mt-3.5 grid grid-cols-[repeat(auto-fill,minmax(min(100%,224px),1fr))] gap-[11px] border-t border-aurora-border-default/50 pt-3.5">{bundleGroups.map(group => <label key={group} className="flex min-w-0 flex-col gap-[7px] rounded-xl border border-dashed border-aurora-border-default bg-aurora-control-surface p-3"><span className="flex justify-between text-[10.5px] font-bold uppercase tracking-wide text-aurora-accent-primary"><span>{group}</span><span>{(bundleEntries[group] ?? '').split('\n').filter(value => value.trim()).length}</span></span><Textarea aria-label={`${group} bundle references`} value={bundleEntries[group] ?? ''} onChange={event => { markDraftDirty(); setBundleEntries(current => ({ ...current, [group]: event.target.value })) }} placeholder="Add artifact references…" className="aurora-scrollbar min-h-20 w-full resize-y rounded-lg border border-aurora-border-default/55 bg-aurora-page-bg p-2 text-xs outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary" /></label>)}</div>
        </section>}
      </div>
    </div></div>
  </>
}
