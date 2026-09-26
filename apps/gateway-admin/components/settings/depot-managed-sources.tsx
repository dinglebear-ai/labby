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

function cadenceSeconds(value: number, unit: CadenceUnit): number {
  const multiplier = unit === 'days' ? 86_400 : unit === 'hours' ? 3_600 : 60
  return value * multiplier
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
          max={525_600}
          value={value}
          disabled={disabled}
          onChange={(event) => setValue(Math.max(1, Number(event.target.value) || 1))}
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
      <Button size="sm" variant="outline" disabled={disabled} onClick={() => void onSave(cadenceSeconds(value, unit))}>
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
  const [credential, setCredential] = useState('')
  const [cadenceValue, setCadenceValue] = useState(1)
  const [cadenceUnit, setCadenceUnit] = useState<CadenceUnit>('days')

  async function load(signal?: AbortSignal): Promise<void> {
    setError(undefined)
    try {
      setSources(await depotSources(signal))
    } catch (reason) {
      if (!signal?.aborted) setError(reason instanceof Error ? reason.message : 'Depot sources unavailable')
    } finally {
      if (!signal?.aborted) setLoading(false)
    }
  }

  useEffect(() => {
    const controller = new AbortController()
    void load(controller.signal)
    return () => controller.abort()
  }, [])

  async function mutate(key: string, action: () => Promise<unknown>): Promise<void> {
    setBusy(key)
    setError(undefined)
    try {
      await action()
      await load()
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : 'Depot source action failed')
    } finally {
      setBusy(undefined)
    }
  }

  async function addRepository(event: React.FormEvent): Promise<void> {
    event.preventDefault()
    const trimmed = url.trim()
    if (!trimmed) {
      setError('Repository URL is required.')
      return
    }
    await mutate('add', () => addDepotRepoSource({
      url: trimmed,
      namespace: namespace.trim() || undefined,
      ref: ref.trim() || undefined,
      subdir: subdir.trim() || undefined,
      credential: credential.trim() || undefined,
      intervalSeconds: cadenceSeconds(cadenceValue, cadenceUnit),
    }))
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
        description="Add a repository once. Depot discovers every SKILL.md across the tree, ingests immediately, then refreshes only changed projections on an anchored schedule."
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
              <Input className="h-8 w-20" type="number" min={1} value={cadenceValue} onChange={(event) => setCadenceValue(Math.max(1, Number(event.target.value) || 1))} />
            </label>
            <label className="space-y-1 text-[11px] text-aurora-text-muted">
              <span className="block">Cadence</span>
              <select className="h-8 rounded-md border border-aurora-border-subtle bg-transparent px-2 text-xs text-aurora-text-primary" value={cadenceUnit} onChange={(event) => setCadenceUnit(event.target.value as CadenceUnit)}>
                <option value="minutes">minutes</option>
                <option value="hours">hours</option>
                <option value="days">days</option>
              </select>
            </label>
            <span className="pb-2 text-[11px] text-aurora-text-muted">Default is 1 day, anchored to initial registration.</span>
          </div>
          <details className="rounded-md border border-aurora-border-subtle p-3">
            <summary className="cursor-pointer text-xs font-medium text-aurora-text-primary">Advanced repository options</summary>
            <div className="mt-3 grid gap-3 md:grid-cols-2">
              <label className="space-y-1 text-[11px] text-aurora-text-muted"><span className="block">Namespace override</span><Input value={namespace} onChange={(event) => setNamespace(event.target.value)} /></label>
              <label className="space-y-1 text-[11px] text-aurora-text-muted"><span className="block">Git ref</span><Input value={ref} onChange={(event) => setRef(event.target.value)} /></label>
              <label className="space-y-1 text-[11px] text-aurora-text-muted"><span className="block">Path filter</span><Input value={subdir} onChange={(event) => setSubdir(event.target.value)} placeholder="Leave blank for full repository" /></label>
              <label className="space-y-1 text-[11px] text-aurora-text-muted"><span className="block">Credential reference</span><Input value={credential} onChange={(event) => setCredential(event.target.value)} placeholder="github-private" /></label>
            </div>
          </details>
          <Button type="submit" size="sm" disabled={busy === 'add'}><Plus className="size-4" />{busy === 'add' ? 'Adding…' : 'Add repository and ingest now'}</Button>
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
                onSave={(seconds) => mutate(key, () => configureDepotSource(source.id, { intervalSeconds: seconds }))}
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
