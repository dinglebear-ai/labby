'use client'

import { useCallback, useEffect, useMemo, useState } from 'react'
import { Archive, BookOpen, Check, Download, FilePlus2, Loader2, Pencil, Plus, RefreshCw, Rocket, Search, Trash2 } from 'lucide-react'
import { toast } from 'sonner'

import { AURORA_DENSE_META, AURORA_MUTED_LABEL } from '@/components/aurora/tokens'
import { DashboardPanel } from '@/components/dashboard/panel'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group'
import { Textarea } from '@/components/ui/textarea'
import {
  skillLibrary,
  type SkillLibraryFile,
  type SkillLibraryItem,
  type SkillLibraryPage,
  type SkillValidation,
  type SkillVisibility,
} from '@/lib/api/skill-library-client'
import { cn, getErrorMessage } from '@/lib/utils'

const STARTER = `---
name: my-skill
description: What this skill helps with
---

# Instructions

Describe when and how to use this skill.
`

function requestKey(action: string) {
  return `gateway-admin-${action}-${crypto.randomUUID()}`
}

function lifecycle(item: SkillLibraryItem, libraryPublished: boolean) {
  if (item.archived) return 'Archived'
  if (!item.active_revision_id) return 'Stored'
  return libraryPublished ? 'Published' : 'Publishing'
}

function LifecycleRail({ selected, validation, libraryPublished = false }: { selected?: SkillLibraryItem; validation?: SkillValidation | null; libraryPublished?: boolean }) {
  const states = [
    ['Stored', Boolean(selected)],
    ['Validated', validation?.valid === true || Boolean(selected)],
    ['Active', Boolean(selected?.active_revision_id)],
    ['Published', Boolean(selected?.active_revision_id) && libraryPublished],
  ] as const
  return (
    <div className="grid grid-cols-4 overflow-hidden rounded-lg border border-aurora-border-subtle bg-aurora-surface-muted/30">
      {states.map(([label, complete], index) => (
        <div key={label} className={cn('relative px-3 py-3', index > 0 && 'border-l border-aurora-border-subtle')}>
          <div className={cn('mb-1 flex size-5 items-center justify-center rounded-full border text-[10px]', complete ? 'border-aurora-success bg-aurora-success/15 text-aurora-success' : 'border-aurora-border-strong text-aurora-text-muted')}>
            {complete ? <Check className="size-3" /> : index + 1}
          </div>
          <span className={AURORA_MUTED_LABEL}>{label}</span>
        </div>
      ))}
    </div>
  )
}

export function SkillLibraryPageContent() {
  const [page, setPage] = useState<SkillLibraryPage | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [editing, setEditing] = useState(false)
  const [name, setName] = useState('my-skill')
  const [visibility, setVisibility] = useState<SkillVisibility>('private')
  const [files, setFiles] = useState<SkillLibraryFile[]>([{ path: 'SKILL.md', content: STARTER }])
  const [activeFile, setActiveFile] = useState(0)
  const [validation, setValidation] = useState<SkillValidation | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [query, setQuery] = useState('')
  const [appliedQuery, setAppliedQuery] = useState('')
  const [importing, setImporting] = useState(false)
  const [importKind, setImportKind] = useState<'repository' | 'depot'>('repository')
  const [importConnection, setImportConnection] = useState('')
  const [importArtifactId, setImportArtifactId] = useState('')
  const [importRevisionId, setImportRevisionId] = useState('')

  const load = useCallback(async (signal?: AbortSignal, search = '') => {
    setLoading(true)
    setError(null)
    try {
      const next = await skillLibrary.list(search, signal)
      setPage(next)
      setSelectedId(current => current && next.items.some(item => item.artifact_id === current) ? current : next.items[0]?.artifact_id ?? null)
    } catch (cause) {
      if (!signal?.aborted) setError(getErrorMessage(cause, 'Unable to load the Artifact Library.'))
    } finally {
      if (!signal?.aborted) setLoading(false)
    }
  }, [])

  useEffect(() => {
    const controller = new AbortController()
    void load(controller.signal)
    return () => controller.abort()
  }, [load])

  const selected = useMemo(() => page?.items.find(item => item.artifact_id === selectedId), [page, selectedId])
  const file = files[activeFile]

  function newSkill() {
    setSelectedId(null)
    setEditing(true)
    setName('my-skill')
    setVisibility(page?.create_visibilities[0] ?? 'private')
    setFiles([{ path: 'SKILL.md', content: STARTER }])
    setActiveFile(0)
    setValidation(null)
  }

  async function editLatest() {
    if (!selected) return
    setBusy('load-revision')
    try {
      const contents = await Promise.all(selected.latest_revision_files.map(async revisionFile => {
        const loaded = await skillLibrary.read(selected.artifact_id, selected.latest_revision_id, revisionFile.path)
        return { path: loaded.path, content: loaded.text }
      }))
      setName(selected.name)
      setVisibility(selected.visibility)
      setFiles(contents)
      setActiveFile(0)
      setValidation(null)
      setEditing(true)
    } catch (cause) {
      toast.error(getErrorMessage(cause, 'Unable to load the latest revision.'))
    } finally {
      setBusy(null)
    }
  }

  async function validate() {
    setBusy('validate')
    try {
      const result = await skillLibrary.validate(name.trim(), files)
      setValidation(result)
      if (result.valid) toast.success('Skill is valid')
    } catch (cause) {
      toast.error(getErrorMessage(cause, 'Unable to validate this Skill.'))
    } finally {
      setBusy(null)
    }
  }

  async function save() {
    if (!page) return
    setBusy('save')
    try {
      const checked = await skillLibrary.validate(name.trim(), files)
      setValidation(checked)
      if (!checked.valid) return
      const receipt = selected
        ? await skillLibrary.save({
            artifactId: selected.artifact_id,
            revisionId: selected.latest_revision_id,
            files,
            expectedLibraryVersion: page.library_version,
            idempotencyKey: requestKey('save'),
          })
        : await skillLibrary.create({
            name: name.trim(), files, visibility,
            expectedLibraryVersion: page.library_version,
            idempotencyKey: requestKey('create'),
          })
      toast.success('Immutable revision saved', { description: 'Activate it when you are ready to publish.' })
      setEditing(false)
      await load(undefined, appliedQuery)
      setSelectedId(receipt.artifact_id)
    } catch (cause) {
      toast.error(getErrorMessage(cause, 'Unable to save this Skill.'))
      await load(undefined, appliedQuery)
    } finally {
      setBusy(null)
    }
  }

  async function activate() {
    if (!page || !selected) return
    setBusy('activate')
    try {
      await skillLibrary.activate({
        artifactId: selected.artifact_id,
        revisionId: selected.latest_revision_id,
        expectedLibraryVersion: page.library_version,
        idempotencyKey: requestKey('activate'),
      })
      toast.success('Skill published')
      await load(undefined, appliedQuery)
    } catch (cause) {
      toast.error(getErrorMessage(cause, 'Unable to activate this Skill.'))
      await load(undefined, appliedQuery)
    } finally {
      setBusy(null)
    }
  }

  async function archive() {
    if (!page || !selected || !window.confirm(`Archive ${selected.name}?`)) return
    setBusy('archive')
    try {
      await skillLibrary.archive({
        artifactId: selected.artifact_id,
        expectedLibraryVersion: page.library_version,
        idempotencyKey: requestKey('archive'),
      })
      toast.success('Artifact archived')
      await load(undefined, appliedQuery)
    } catch (cause) {
      toast.error(getErrorMessage(cause, 'Unable to archive this Artifact.'))
      await load(undefined, appliedQuery)
    } finally {
      setBusy(null)
    }
  }

  async function importArtifact() {
    if (!page) return
    setBusy('import')
    try {
      const source = importKind === 'repository'
        ? { kind: 'repository' as const, connection_id: importConnection.trim(), artifact_id: importArtifactId.trim(), revision_id: importRevisionId.trim() }
        : { kind: 'depot' as const, connection_id: importConnection.trim(), artifact_id: importArtifactId.trim(), revision_id: importRevisionId.trim() }
      const receipt = await skillLibrary.import({
        source,
        expectedLibraryVersion: page.library_version,
        idempotencyKey: requestKey('import'),
      })
      toast.success('Artifact imported', { description: 'The exact provider revision is now stored in this Labby library.' })
      setImporting(false)
      await load(undefined, appliedQuery)
      setSelectedId(receipt.artifact_id)
    } catch (cause) {
      toast.error(getErrorMessage(cause, 'Unable to import this Artifact.'))
      await load(undefined, appliedQuery)
    } finally {
      setBusy(null)
    }
  }

  return (
    <div className="grid gap-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div><h2 className="font-display text-xl font-semibold">Artifact Library</h2><p className={cn(AURORA_DENSE_META, 'mt-1 text-aurora-text-muted')}>Durable, revisioned artifacts owned by Labby. Agent Skills are the first supported kind.</p></div>
        <div className="flex gap-2"><Button variant="outline" size="sm" onClick={() => void load(undefined, appliedQuery)} disabled={loading}><RefreshCw className={cn('size-4', loading && 'animate-spin')} />Refresh</Button><Button variant="outline" size="sm" onClick={() => { setImporting(true); setEditing(false) }} disabled={!page}><Download className="size-4" />Import</Button><Button size="sm" onClick={newSkill} disabled={!page?.can_create}><Plus className="size-4" />Create skill</Button></div>
      </div>

      {error ? <DashboardPanel title="Library unavailable"><p className="text-sm text-destructive">{error}</p></DashboardPanel> : null}
      {loading && !page ? <div className="flex min-h-56 items-center justify-center"><Loader2 className="size-5 animate-spin" /></div> : null}

      {page ? <div className="grid min-h-[520px] gap-3 lg:grid-cols-[260px_minmax(0,1fr)]">
        <DashboardPanel title="Artifacts" meta={`${page.items.length}`}>
          <form className="relative mb-3" onSubmit={event => { event.preventDefault(); const next = query.trim(); setAppliedQuery(next); void load(undefined, next) }}>
            <Search className="pointer-events-none absolute left-2.5 top-2.5 size-4 text-aurora-text-muted" />
            <Input aria-label="Search artifacts" className="pl-8" value={query} onChange={event => setQuery(event.target.value)} placeholder="Search artifacts" />
          </form>
          <div className="grid gap-1">
            {page.items.length === 0 ? <div className="py-10 text-center"><BookOpen className="mx-auto mb-3 size-6 text-aurora-text-muted" /><p className="text-sm">No managed Skills yet.</p><Button className="mt-4" size="sm" onClick={newSkill}>Create the first Skill</Button></div> : page.items.map(item => <button key={item.artifact_id} onClick={() => { setSelectedId(item.artifact_id); setEditing(false); setValidation(null) }} className={cn('rounded-lg border px-3 py-2.5 text-left transition-colors', selectedId === item.artifact_id ? 'border-aurora-accent-primary bg-aurora-accent-primary/10' : 'border-transparent hover:border-aurora-border-subtle hover:bg-aurora-surface-muted/40')}><span className="block truncate text-sm font-medium">{item.name}</span><span className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>{lifecycle(item, page.library_version === page.published_library_version)} · {item.visibility === 'shared' ? 'Shared' : 'Personal'}</span></button>)}
          </div>
        </DashboardPanel>

        <DashboardPanel title={importing ? 'Import exact Artifact' : editing ? selected ? `Revise ${selected.name}` : 'Create a Skill' : selected?.name ?? 'Select a Skill'}>
          {importing ? <div className="grid gap-4">
            <p className="text-sm text-aurora-text-muted">Import an immutable revision through a source configured by the Labby operator. Endpoints and credentials never enter the browser.</p>
            <fieldset className="grid gap-1.5"><legend className={AURORA_MUTED_LABEL}>Source type</legend><RadioGroup value={importKind} onValueChange={value => setImportKind(value as 'repository' | 'depot')} className="flex min-h-9 items-center gap-5"><label className="flex items-center gap-2 text-sm"><RadioGroupItem value="repository" />Repository</label><label className="flex items-center gap-2 text-sm"><RadioGroupItem value="depot" />Depot</label></RadioGroup></fieldset>
            <label className="grid gap-1.5"><span className={AURORA_MUTED_LABEL}>{importKind === 'repository' ? 'Repository connection' : 'Depot connection'}</span><Input aria-label="Import connection" value={importConnection} onChange={event => setImportConnection(event.target.value)} placeholder="team-skills" /></label>
            <label className="grid gap-1.5"><span className={AURORA_MUTED_LABEL}>Artifact ID</span><Input aria-label="Import artifact ID" value={importArtifactId} onChange={event => setImportArtifactId(event.target.value)} /></label>
            <label className="grid gap-1.5"><span className={AURORA_MUTED_LABEL}>{importKind === 'repository' ? 'Exact object ID' : 'Exact revision ID'}</span><Input aria-label="Import revision ID" className="font-mono text-xs" value={importRevisionId} onChange={event => setImportRevisionId(event.target.value)} placeholder="sha256:…" /></label>
            <div className="flex justify-end gap-2"><Button variant="outline" onClick={() => setImporting(false)}>Cancel</Button><Button onClick={() => void importArtifact()} disabled={busy !== null || !importConnection.trim() || !importArtifactId.trim() || !importRevisionId.trim()}>{busy === 'import' ? <Loader2 className="size-4 animate-spin" /> : <Download className="size-4" />}Import exact revision</Button></div>
          </div> : editing ? <div className="grid gap-4">
            <LifecycleRail validation={validation} />
            {validation && !validation.valid ? <div className="rounded-lg border border-amber-500/40 bg-amber-500/10 p-3"><p className="text-sm font-medium text-amber-500">Validation needs attention</p><ul className={cn(AURORA_DENSE_META, 'mt-2 list-disc pl-4 text-aurora-text-muted')}>{validation.rejections.map((item, index) => <li key={`${item.code}-${index}`}>{item.path ? `${item.path}: ` : ''}{item.field} · {item.code}</li>)}</ul></div> : null}
            <div className="grid gap-4 sm:grid-cols-2"><label className="grid gap-1.5"><span className={AURORA_MUTED_LABEL}>Skill name</span><Input aria-label="Skill name" value={name} disabled={Boolean(selected)} onChange={event => setName(event.target.value)} /></label><fieldset className="grid gap-1.5" disabled={Boolean(selected)}><legend className={AURORA_MUTED_LABEL}>Save to</legend><RadioGroup value={visibility} onValueChange={value => setVisibility(value as SkillVisibility)} className="flex min-h-9 items-center gap-5">{page.create_visibilities.map(value => <label key={value} className="flex items-center gap-2 text-sm"><RadioGroupItem value={value} />{value === 'shared' ? 'Shared library' : 'Personal library'}</label>)}</RadioGroup></fieldset></div>
            <div className="flex flex-wrap items-center gap-2">{files.map((item, index) => <Button key={`${item.path}-${index}`} variant={activeFile === index ? 'secondary' : 'ghost'} size="sm" onClick={() => setActiveFile(index)}>{item.path}</Button>)}<Button variant="outline" size="sm" onClick={() => { setFiles(current => [...current, { path: 'references/notes.md', content: '' }]); setActiveFile(files.length) }}><FilePlus2 className="size-4" />Supporting file</Button>{files.length > 1 ? <Button variant="ghost" size="icon-sm" aria-label="Remove current file" onClick={() => { setFiles(current => current.filter((_, index) => index !== activeFile)); setActiveFile(0) }}><Trash2 className="size-4" /></Button> : null}</div>
            {file ? <><label className="grid gap-1.5"><span className={AURORA_MUTED_LABEL}>Logical file name</span><Input value={file.path} onChange={event => setFiles(current => current.map((item, index) => index === activeFile ? { ...item, path: event.target.value } : item))} /></label><label className="grid gap-1.5"><span className={AURORA_MUTED_LABEL}>Contents</span><Textarea className="min-h-72 font-mono text-xs leading-relaxed" value={file.content} onChange={event => setFiles(current => current.map((item, index) => index === activeFile ? { ...item, content: event.target.value } : item))} /></label></> : null}
            <div className="flex justify-end gap-2"><Button variant="outline" onClick={() => setEditing(false)}>Cancel</Button><Button variant="secondary" onClick={() => void validate()} disabled={busy !== null}>{busy === 'validate' ? <Loader2 className="size-4 animate-spin" /> : <Check className="size-4" />}Validate</Button><Button onClick={() => void save()} disabled={busy !== null || !name.trim()}>{busy === 'save' ? <Loader2 className="size-4 animate-spin" /> : null}Save immutable revision</Button></div>
          </div> : selected ? <div className="grid gap-5"><LifecycleRail selected={selected} libraryPublished={page.library_version === page.published_library_version} /><div className="flex flex-wrap gap-2"><Badge variant="outline">{selected.visibility === 'shared' ? 'Shared' : 'Personal'}</Badge><Badge variant="outline">{selected.provenance.source}</Badge><Badge variant="outline">{selected.latest_revision_files.length} files</Badge></div><div className="grid gap-3 sm:grid-cols-2"><div><p className={AURORA_MUTED_LABEL}>Published URI</p><code className="mt-1 block break-all text-xs">{selected.canonical_uri ?? 'Not published'}</code></div><div><p className={AURORA_MUTED_LABEL}>Latest revision</p><code className="mt-1 block break-all text-xs">{selected.latest_revision_id}</code></div></div><div className="flex flex-wrap justify-end gap-2"><Button variant="outline" onClick={() => void editLatest()} disabled={busy !== null || !selected.allowed_actions.includes('artifacts.save')}><Pencil className="size-4" />Edit latest</Button><Button variant="destructive" onClick={() => void archive()} disabled={busy !== null || !selected.allowed_actions.includes('artifacts.archive')}><Archive className="size-4" />Archive</Button><Button onClick={() => void activate()} disabled={busy !== null || selected.active_revision_id === selected.latest_revision_id || !selected.allowed_actions.includes('artifacts.activate')}><Rocket className="size-4" />{busy === 'activate' ? 'Publishing…' : 'Activate latest revision'}</Button></div></div> : <div className="flex min-h-72 items-center justify-center text-sm text-aurora-text-muted">Select a Skill or create a new one.</div>}
        </DashboardPanel>
      </div> : null}
    </div>
  )
}
