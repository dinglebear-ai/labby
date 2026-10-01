'use client'

import { useEffect, useMemo, useState } from 'react'
import { Clock3, Loader2, Pause, Play, Plus, RefreshCw, Trash2 } from 'lucide-react'

import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  addDepotRepoSource,
  configureDepotSource,
  deleteDepotSource,
  depotSources,
  depotGitCredentialChoices,
  type DepotGitCredentialChoice,
  refreshDepotSource,
  type DepotSource,
} from '@/lib/api/depot-client'
import { SettingsCard } from './SettingsChrome'

type CadenceUnit = 'minutes' | 'hours' | 'days'

function cadenceParts(seconds: number): { value: number; unit: CadenceUnit } {
  if (seconds % 86_400 === 0) return { value: seconds / 86_400, unit: 'days' }
  if (seconds % 3_600 === 0) return { value: seconds / 3_600, unit: 'hours' }
  return { value: Math.max(1, Math.round(seconds / 60)), unit: 'minutes' }
}

export function cadenceSeconds(value: number, unit: CadenceUnit): number {
  const multiplier = unit === 'days' ? 86_400 : unit === 'hours' ? 3_600 : 60
  return Number.isSafeInteger(value) && value >= 1 && Number.isSafeInteger(value * multiplier) && value * multiplier <= 31_536_000 ? value * multiplier : NaN
}

export function repositoryInputError(url: string, namespace: string, ref: string, subdir: string): string | undefined {
  try {
    const parsed = new URL(url.trim())
    if (parsed.protocol !== 'https:' || parsed.username || parsed.password || !parsed.hostname) return 'Enter an HTTPS repository URL without an embedded username or password.'
  } catch { return 'Enter a valid HTTPS repository URL.' }
  if (namespace.trim() && (!/^[a-z0-9]+(-[a-z0-9]+)*$/.test(namespace.trim()) || namespace.trim().length > 64)) return 'Use at most 64 lowercase letters, digits, and single hyphens for the catalog namespace.'
  const revision = ref.trim()
  if (revision && (revision.startsWith('-') || Array.from(revision).some((char) => char.charCodeAt(0) <= 32 || char.charCodeAt(0) === 127 || '~^:?*[\\'.includes(char)) || revision.includes('..') || revision.includes('@{') || revision.includes('//') || revision.endsWith('/') || revision.endsWith('.') || revision.split('/').some((part) => part.startsWith('.') || part.endsWith('.lock')))) return 'Enter a Git branch, tag, or commit without spaces, control characters, or Git revision expressions.'
  const path = subdir.trim()
  if (path && (path.startsWith('/') || path.includes('\\') || Array.from(path).some((char) => char.charCodeAt(0) < 32 || char.charCodeAt(0) === 127) || path.split('/').some((part) => part === '..' || part === '.'))) return 'Use a relative repository subdirectory without dot segments or backslashes.'
  return undefined
}

function sourceLabel(source: DepotSource): string {
  for (const key of ['url', 'source', 'endpoint', 'domain']) {
    const value = source.args[key]
    if (typeof value === 'string' && value) return value
  }
  return source.id
}

function formatWhen(value?: string): string {
  if (!value) return 'never'
  const parsed = new Date(value)
  return Number.isNaN(parsed.valueOf()) ? value : parsed.toLocaleString()
}

function deltaSummary(event?: NonNullable<DepotSource['history']>[number]): string {
  if (!event) return 'No completed ingest yet'
  if (event.status === 'failed') return event.error ? `Failed: ${event.error}` : 'Failed'
  return `+${event.added?.length ?? 0} ~${event.changed?.length ?? 0} −${event.removed?.length ?? 0}`
}

function SourceCadence({
  source,
  disabled,
  onSave,
}: {
  source: DepotSource
  disabled: boolean
  onSave: (seconds: number) => Promise<void>
}): React.ReactElement {
  const initial = useMemo(() => cadenceParts(source.intervalSeconds), [source.intervalSeconds])
  const [value, setValue] = useState(initial.value)
  const [unit, setUnit] = useState<CadenceUnit>(initial.unit)

  useEffect(() => {
    setValue(initial.value)
    setUnit(initial.unit)
  }, [initial])

  return (
    <div className="flex flex-wrap items-end gap-2">
      <label className="space-y-1 text-[11px] text-aurora-text-muted">
        <span className="block">Refresh every</span>
        <Input
          className="h-8 w-20"
          type="number"
          min={1}
          step={1}
          value={Number.isFinite(value) ? value : ''}
          disabled={disabled}
          onChange={(event) => setValue(event.target.value === '' ? NaN : Number(event.target.value))}
        />
      </label>
      <label className="space-y-1 text-[11px] text-aurora-text-muted">
        <span className="block">Cadence</span>
        <select
          className="h-8 rounded-md border border-aurora-border-subtle bg-transparent px-2 text-xs text-aurora-text-primary"
          value={unit}
          disabled={disabled}
          onChange={(event) => setUnit(event.target.value as CadenceUnit)}
        >
          <option value="minutes">minutes</option>
          <option value="hours">hours</option>
          <option value="days">days</option>
        </select>
      </label>
      <Button size="sm" variant="outline" disabled={disabled || !Number.isFinite(cadenceSeconds(value, unit))} onClick={() => void onSave(cadenceSeconds(value, unit))}>
        Set cadence
      </Button>
    </div>
  )
}

export function DepotManagedSources(): React.ReactElement {
  const [sources, setSources] = useState<DepotSource[]>([])
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState<string>()
  const [error, setError] = useState<string>()
  const [confirmDelete, setConfirmDelete] = useState<string>()
  const [url, setUrl] = useState('')
  const [namespace, setNamespace] = useState('')
  const [ref, setRef] = useState('')
  const [subdir, setSubdir] = useState('')
  const [credentialChoices, setCredentialChoices] = useState<DepotGitCredentialChoice[]>([])
  const [credentialError, setCredentialError] = useState<string>()
  const [credential, setCredential] = useState('')
  const [cadenceValue, setCadenceValue] = useState(1)
  const [cadenceUnit, setCadenceUnit] = useState<CadenceUnit>('days')

  const repositoryHost = useMemo(() => {
    try { const parsed = new URL(url.trim()); return parsed.protocol === 'https:' && !parsed.username && !parsed.password ? parsed.hostname.toLowerCase() : undefined } catch { return undefined }
  }, [url])
  const matchingCredentials = credentialChoices.filter((choice) => choice.host.toLowerCase() === repositoryHost)

  useEffect(() => {
    if (credential && !matchingCredentials.some((choice) => choice.id === credential)) setCredential('')
  }, [credential, matchingCredentials])

  async function load(signal?: AbortSignal): Promise<boolean> {
    setError(undefined)
    try {
      setSources(await depotSources(signal))
      return true
    } catch (reason) {
      if (!signal?.aborted) setError(reason instanceof Error ? reason.message : 'Depot sources unavailable')
      return false
    } finally {
      if (!signal?.aborted) setLoading(false)
    }
  }

  useEffect(() => {
    const controller = new AbortController()
    void load(controller.signal)
    void depotGitCredentialChoices(controller.signal).then((choices) => {
      if (!controller.signal.aborted) { setCredentialChoices(choices); setCredentialError(undefined) }
    }).catch(() => {
      if (!controller.signal.aborted) setCredentialError('Saved repository credentials are unavailable. The catalog must support credential choices and your account needs ingestion write access. Public repositories can use no credential.')
    })
    return () => controller.abort()
  }, [])

  async function mutate(key: string, action: () => Promise<unknown>): Promise<boolean> {
    setBusy(key)
    setError(undefined)
    try {
      await action()
      if (!(await load())) {
        setError('Action succeeded, but the source list could not refresh. Refresh the list before retrying.')
      }
      return true
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : 'Depot source action failed')
      return false
    } finally {
      setBusy(undefined)
    }
  }

  async function addRepository(event: React.FormEvent): Promise<void> {
    event.preventDefault()
    if (!Number.isFinite(cadenceSeconds(cadenceValue, cadenceUnit))) { setError('Choose a refresh interval of one or more whole minutes, hours, or days.'); return }
    const validationError = repositoryInputError(url, namespace, ref, subdir)
    if (validationError) { setError(validationError); return }
    if (credential && !matchingCredentials.some((choice) => choice.id === credential)) { setError('Choose a saved credential matching this repository host.'); return }
    const trimmed = url.trim()
    if (!repositoryHost) {
      setError('Enter an HTTPS repository URL without an embedded username or password.')
      return
    }
    const added = await mutate('add', () => addDepotRepoSource({
      url: trimmed,
      namespace: namespace.trim() || undefined,
      ref: ref.trim() || undefined,
      subdir: subdir.trim() || undefined,
      credential: credential.trim() || undefined,
      intervalSeconds: cadenceSeconds(cadenceValue, cadenceUnit),
    }))
    if (!added) return
    setUrl('')
    setNamespace('')
    setRef('')
    setSubdir('')
    setCredential('')
  }

  return (
    <div className="space-y-4">
      <SettingsCard
        title="Managed repositories"
        description="Register a Git repository for the catalog server to scan for Skills. It reads SKILL.md files immediately, then checks for changes at the interval you choose. This does not install Skills or run MCP servers."
        action={<Button size="sm" variant="outline" disabled={loading} onClick={() => void load()}><RefreshCw className="size-4" />Refresh</Button>}
      >
        <form className="space-y-3 p-4" onSubmit={(event) => void addRepository(event)}>
          <label className="space-y-1 text-xs text-aurora-text-muted">
            <span className="block font-medium text-aurora-text-primary">Repository URL</span>
            <Input value={url} onChange={(event) => setUrl(event.target.value)} placeholder="https://github.com/dinglebear-ai/limetech-marketplace" />
          </label>
          <div className="flex flex-wrap items-end gap-2">
            <label className="space-y-1 text-[11px] text-aurora-text-muted">
              <span className="block">Refresh every</span>
              <Input className="h-8 w-20" type="number" min={1} step={1} value={Number.isFinite(cadenceValue) ? cadenceValue : ''} onChange={(event) => setCadenceValue(event.target.value === '' ? NaN : Number(event.target.value))} />
            </label>
            <label className="space-y-1 text-[11px] text-aurora-text-muted">
              <span className="block">Cadence</span>
              <select className="h-8 rounded-md border border-aurora-border-subtle bg-transparent px-2 text-xs text-aurora-text-primary" value={cadenceUnit} onChange={(event) => setCadenceUnit(event.target.value as CadenceUnit)}>
                <option value="minutes">minutes</option>
                <option value="hours">hours</option>
                <option value="days">days</option>
              </select>
            </label>
            <span className="pb-2 text-[11px] text-aurora-text-muted">Whole numbers only, up to 365 days. The schedule starts when this repository is registered.</span>
          </div>
          <details className="rounded-md border border-aurora-border-subtle p-3">
            <summary className="cursor-pointer text-xs font-medium text-aurora-text-primary">Advanced repository options</summary>
            <div className="mt-3 grid gap-3 md:grid-cols-2">
              <label className="space-y-1 text-[11px] text-aurora-text-muted"><span className="block">Catalog namespace (optional)</span><span className="block">Groups imported Skills. Defaults to the repository name; use lowercase letters, digits, and hyphens.</span><Input value={namespace} onChange={(event) => setNamespace(event.target.value)} /></label>
              <label className="space-y-1 text-[11px] text-aurora-text-muted"><span className="block">Git branch, tag, or commit (optional)</span><span className="block">Chooses the revision the catalog scans. Blank follows the default branch; a commit pins the content.</span><Input value={ref} onChange={(event) => setRef(event.target.value)} /></label>
              <label className="space-y-1 text-[11px] text-aurora-text-muted"><span className="block">Repository subdirectory (optional)</span><span className="block">Restricts discovery to a relative path inside this repository.</span><Input value={subdir} onChange={(event) => setSubdir(event.target.value)} placeholder="Leave blank for full repository" /></label>
              <label className="space-y-1 text-[11px] text-aurora-text-muted"><span className="block">Saved catalog credential ID (optional)</span><select className="h-9 w-full rounded-md border border-aurora-border-subtle bg-transparent px-2 text-xs text-aurora-text-primary" value={credential} onChange={(event) => setCredential(event.target.value)}><option value="">No credential (public repository)</option>{matchingCredentials.map((choice) => <option key={choice.id} value={choice.id}>{choice.id} — {choice.host}</option>)}</select><span className="block">Only saved credentials matching this repository host are offered. The catalog server uses the secret; it is never sent to this browser.</span>{credentialError ? <span role="status" className="block">{credentialError}</span> : null}</label>
            </div>
          </details>
          <Button type="submit" size="sm" disabled={busy === 'add' || !!repositoryInputError(url, namespace, ref, subdir) || !Number.isFinite(cadenceSeconds(cadenceValue, cadenceUnit))}><Plus className="size-4" />{busy === 'add' ? 'Adding…' : 'Add repository and ingest now'}</Button>
        </form>
        {error ? <p role="alert" className="border-t border-aurora-border-subtle p-4 text-xs text-aurora-error">{error}</p> : null}
        {loading && sources.length === 0 ? <p className="flex items-center gap-2 border-t border-aurora-border-subtle p-4 text-xs text-aurora-text-muted"><Loader2 className="size-4 animate-spin" />Loading sources…</p> : null}
        {!loading && sources.length === 0 ? <p className="border-t border-aurora-border-subtle p-4 text-xs text-aurora-text-muted">No managed sources configured.</p> : null}
        {sources.map((source) => {
          const latest = source.history?.[0]
          const key = source.id
          const working = busy === key
          return (
            <div key={source.id} className="space-y-3 border-t border-aurora-border-subtle p-4">
              <div className="flex flex-wrap items-start justify-between gap-3">
                <div className="min-w-0">
                  <strong className="break-all text-sm text-aurora-text-primary">{sourceLabel(source)}</strong>
                  <div className="mt-1 flex flex-wrap gap-2 text-[11px] text-aurora-text-muted">
                    <span>{source.kind}</span>
                    {typeof source.args.namespace === 'string' ? <span>namespace: {source.args.namespace}</span> : null}
                    <span>{source.id}</span>
                  </div>
                </div>
                <div className="flex gap-2">
                  <Badge variant="outline">{source.enabled ? 'enabled' : 'paused'}</Badge>
                  {source.lastError ? <Badge variant="outline" className="text-aurora-error">failing</Badge> : <Badge variant="outline">healthy</Badge>}
                </div>
              </div>
              <div className="grid gap-3 text-xs md:grid-cols-3">
                <div><span className="block text-aurora-text-muted">Anchor</span>{formatWhen(source.scheduleAnchorAt ?? source.insertedAt)}</div>
                <div><span className="block text-aurora-text-muted">Next run</span>{formatWhen(source.nextAttemptAt)}</div>
                <div><span className="block text-aurora-text-muted">Latest ingest</span>{deltaSummary(latest)}</div>
              </div>
              {source.lastError ? <p className="rounded-md border border-aurora-error/30 bg-aurora-error/5 p-2 text-xs text-aurora-error">{source.lastError}</p> : null}
              <SourceCadence
                source={source}
                disabled={working}
                onSave={async (seconds) => { await mutate(key, () => configureDepotSource(source.id, { intervalSeconds: seconds })) }}
              />
              <div className="flex flex-wrap gap-2">
                <Button size="sm" variant="outline" disabled={working} onClick={() => void mutate(key, () => refreshDepotSource(source.id))}><RefreshCw className="size-4" />Run now</Button>
                <Button size="sm" variant="outline" disabled={working} onClick={() => void mutate(key, () => configureDepotSource(source.id, { enabled: !source.enabled }))}>
                  {source.enabled ? <Pause className="size-4" /> : <Play className="size-4" />}{source.enabled ? 'Pause' : 'Enable'}
                </Button>
                <Button
                  size="sm"
                  variant={confirmDelete === source.id ? 'default' : 'outline'}
                  disabled={working}
                  onClick={() => {
                    if (confirmDelete !== source.id) {
                      setConfirmDelete(source.id)
                      return
                    }
                    setConfirmDelete(undefined)
                    void mutate(key, () => deleteDepotSource(source.id))
                  }}
                >
                  <Trash2 className="size-4" />{confirmDelete === source.id ? 'Confirm delete + withdraw skills' : 'Delete source'}
                </Button>
              </div>
              <details>
                <summary className="flex cursor-pointer items-center gap-2 text-xs text-aurora-text-muted"><Clock3 className="size-3.5" />Ingest history ({source.history?.length ?? 0})</summary>
                <div className="mt-2 space-y-2">
                  {(source.history ?? []).slice(0, 10).map((event, index) => (
                    <div key={`${event.at}-${index}`} className="rounded-md border border-aurora-border-subtle p-2 text-[11px]">
                      <div className="flex flex-wrap justify-between gap-2"><strong>{event.status}</strong><span className="text-aurora-text-muted">{formatWhen(event.at)}</span></div>
                      <p className={event.status === 'failed' ? 'mt-1 text-aurora-error' : 'mt-1 text-aurora-text-muted'}>{deltaSummary(event)}</p>
                    </div>
                  ))}
                  {(source.history?.length ?? 0) === 0 ? <p className="text-[11px] text-aurora-text-muted">No recorded runs yet.</p> : null}
                </div>
              </details>
            </div>
          )
        })}
      </SettingsCard>
    </div>
  )
}
