'use client'

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Activity, Boxes, Database, Loader2, Play, RefreshCw, Search, ShieldCheck, Wrench } from 'lucide-react'
import { toast } from 'sonner'

import { AppHeader } from '@/components/app-header'
import { AURORA_MUTED_LABEL, AURORA_PAGE_FRAME } from '@/components/aurora/tokens'
import { ConsoleHero } from '@/components/console/console-hero'
import { AdministrationOverview } from './administration-overview'
import { DashboardPanel } from '@/components/dashboard/panel'
import { initialOperationForm, isDestructiveOperation, operationParams, type OperationFormState, type OperationProperty } from '@/components/depot/operation-form'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import { depotCall, depotOperations, depotStatus, type DepotOperation, type DepotStatus } from '@/lib/api/depot-client'
import { cn, getErrorMessage } from '@/lib/utils'

type Workspace = 'overview' | 'catalog' | 'access' | 'operations'

const WORKSPACES: Array<{ id: Workspace; label: string; icon: typeof Boxes }> = [
  { id: 'overview', label: 'Overview', icon: Activity },
  { id: 'catalog', label: 'Catalog', icon: Boxes },
  { id: 'access', label: 'Access', icon: ShieldCheck },
  { id: 'operations', label: 'Operations', icon: Wrench },
]

function operationWorkspace(operation: DepotOperation): Exclude<Workspace, 'overview'> {
  return operation.group ?? 'catalog'
}

export function OperationGrid({ operations, workspace }: { operations: DepotOperation[]; workspace: Exclude<Workspace, 'overview'> }) {
  const [selected, setSelected] = useState<DepotOperation | null>(null)
  const [form, setForm] = useState<OperationFormState>({})
  const [confirmed, setConfirmed] = useState(false)
  const [running, setRunning] = useState(false)
  const [result, setResult] = useState<{ operation: string; value: unknown } | null>(null)
  const [query, setQuery] = useState('')
  const [limit, setLimit] = useState(48)
  const runGeneration = useRef(0)
  const runController = useRef<AbortController | null>(null)
  const destructiveIntentKey = useRef(crypto.randomUUID())
  const matching = operations.filter(operation => operationWorkspace(operation) === workspace && `${operation.title} ${operation.description} ${operation.name}`.toLowerCase().includes(query.trim().toLowerCase()))
  const visible = matching.slice(0, limit)

  useEffect(() => () => runController.current?.abort(), [])

  const open = (operation: DepotOperation) => {
    runController.current?.abort()
    runGeneration.current += 1
    const properties = operation.inputSchema.properties ?? {}
    setForm(initialOperationForm(properties))
    setConfirmed(false)
    destructiveIntentKey.current = crypto.randomUUID()
    setResult(null)
    setSelected(operation)
  }

  const execute = async () => {
    if (!selected) return
    const operation = selected
    const generation = ++runGeneration.current
    runController.current?.abort()
    const controller = new AbortController()
    runController.current = controller
    setResult(null)
    let parsed: Record<string, unknown>
    try { parsed = operationParams(operation.inputSchema.properties ?? {}, operation.inputSchema.required ?? [], form) }
    catch (error) { toast.error(getErrorMessage(error, 'Review the operation parameters.')); return }
    setRunning(true)
    try {
      const destructive = isDestructiveOperation(operation.annotations)
      const response = await depotCall<{ result: unknown }>(operation.name, parsed, controller.signal, destructive ? { confirmed: true, idempotencyKey: destructiveIntentKey.current } : undefined)
      if (generation !== runGeneration.current) return
      setResult({ operation: operation.name, value: response.result })
      if (destructive) {
        setConfirmed(false)
        destructiveIntentKey.current = crypto.randomUUID()
      }
      toast.success(`${operation.title} completed`)
    } catch (error) {
      if (!controller.signal.aborted && generation === runGeneration.current) toast.error(getErrorMessage(error, `Unable to run ${operation.title}.`))
    } finally { if (generation === runGeneration.current) setRunning(false) }
  }

  const close = () => {
    runController.current?.abort()
    runController.current = null
    runGeneration.current += 1
    setRunning(false)
    setResult(null)
    setSelected(null)
  }

  return <>
    <section aria-label={`${workspace} operation catalog`} className="min-w-0 overflow-hidden rounded-aurora-2 border border-[color-mix(in_srgb,var(--aurora-border-default)_45%,var(--aurora-page-bg))] bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] shadow-[var(--aurora-shadow-medium),inset_0_1px_0_rgba(255,255,255,0.04)]">
    <div className="flex items-center gap-2 border-b border-[color-mix(in_srgb,var(--aurora-border-default)_55%,var(--aurora-page-bg))] px-3 py-2.5">
      <label className="relative min-w-0 flex-1" htmlFor={`depot-operation-search-${workspace}`}><Search aria-hidden="true" className="pointer-events-none absolute left-[11px] top-1/2 size-3.5 -translate-y-1/2 text-aurora-text-muted"/><span className="sr-only">Search {workspace} operations</span><Input id={`depot-operation-search-${workspace}`} value={query} onChange={event => { setQuery(event.target.value); setLimit(48) }} className="h-8 rounded-[9px] border-aurora-border-default bg-aurora-control-surface pl-9 text-[12.5px]" placeholder="Search operations" /></label>
      <span className="whitespace-nowrap text-[11px] text-aurora-text-muted">{matching.length} operations</span>
    </div>
    <div className="grid grid-cols-[repeat(auto-fill,minmax(min(260px,100%),1fr))] gap-2.5 p-3">
      {visible.map(operation => {
        const readOnly = operation.annotations?.readOnlyHint === true
        const destructive = isDestructiveOperation(operation.annotations)
        return <button key={operation.name} type="button" onClick={() => open(operation)} className="group flex min-w-0 flex-col gap-2 rounded-xl border border-[color-mix(in_srgb,var(--aurora-border-default)_60%,var(--aurora-page-bg))] bg-aurora-control-surface p-3.5 text-left transition-colors duration-150 hover:border-[color-mix(in_srgb,var(--aurora-accent-primary)_40%,transparent)] hover:bg-aurora-hover-bg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary">
          <div className="flex w-full items-start justify-between gap-2.5"><span className="font-display text-sm font-[760] text-aurora-text-primary">{operation.title}</span><Badge variant="outline" className={cn('h-[18px] shrink-0 rounded border-[color-mix(in_srgb,currentColor_30%,transparent)] bg-[color-mix(in_srgb,currentColor_11%,transparent)] px-[7px] text-[9px] font-bold uppercase tracking-[.1em]', destructive ? 'text-aurora-error' : readOnly ? 'text-aurora-success' : 'text-aurora-warn')}>{destructive ? 'Destructive' : readOnly ? 'Read' : 'Admin'}</Badge></div>
          <p className="text-pretty text-xs leading-[1.55] text-aurora-text-muted">{operation.description}</p>
          <code className="block max-w-full truncate font-mono text-[10.5px] text-aurora-accent-strong">{operation.name}</code>
        </button>
      })}
    </div>
    {visible.length < matching.length ? <div className="mt-4 flex justify-center"><Button variant="outline" onClick={() => setLimit(current => Math.min(current + 48, matching.length))}>Show more ({matching.length - visible.length} remaining)</Button></div> : null}
    {matching.length === 0 ? <p className="rounded-aurora-2 border border-aurora-border-subtle p-6 text-center text-sm text-aurora-text-muted">No operations match this search.</p> : null}
    </section>
    <Dialog open={Boolean(selected)} onOpenChange={openState => !openState && close()}>
      <DialogContent className="max-h-[min(780px,calc(100vh-2rem))] max-w-2xl overflow-y-auto border-aurora-border-strong bg-aurora-panel-medium">
        <DialogHeader><DialogTitle>{selected?.title}</DialogTitle><DialogDescription>{selected?.description}</DialogDescription></DialogHeader>
        <div className="grid gap-5"><div><p className={AURORA_MUTED_LABEL}>Canonical operation</p><code className="mt-1 block text-sm text-aurora-accent-primary">{selected?.name}</code></div>
          <div className="grid gap-4 sm:grid-cols-2">{Object.entries(selected?.inputSchema.properties ?? {}).map(([name, raw]) => {
            const property: OperationProperty = raw
            const required = selected?.inputSchema.required?.includes(name) ?? false
            const id = `depot-operation-${name}`
            const value = form[name]
            const setValue = (next: string | boolean) => setForm(current => ({ ...current, [name]: next }))
            return <div key={name} className={cn('grid content-start gap-1.5', property.type === 'object' || property.type === 'array' ? 'sm:col-span-2' : '')}>
              <label className="text-sm font-semibold text-aurora-text-primary" htmlFor={id}>{name}{required ? <span className="ml-1 text-aurora-warn">*</span> : null}</label>
              {property.type === 'boolean' && !required ? <Select value={value === undefined ? 'unset' : String(value)} onValueChange={next => setForm(current => ({ ...current, [name]: next === 'unset' ? undefined : next === 'true' }))}><SelectTrigger id={id}><SelectValue /></SelectTrigger><SelectContent><SelectItem value="unset">Use Depot default</SelectItem><SelectItem value="true">Enabled</SelectItem><SelectItem value="false">Disabled</SelectItem></SelectContent></Select>
                : property.type === 'boolean' ? <label className="flex min-h-9 items-center gap-2 rounded-md border border-input px-3 text-sm text-aurora-text-muted" htmlFor={id}><Checkbox id={id} checked={value === true} onCheckedChange={checked => setValue(checked === true)} />Enabled</label>
                : Array.isArray(property.enum) ? <Select value={typeof value === 'string' ? value : ''} onValueChange={setValue}><SelectTrigger id={id}><SelectValue placeholder="Select a value" /></SelectTrigger><SelectContent>{property.enum.map(option => <SelectItem key={String(option)} value={String(option)}>{String(option)}</SelectItem>)}</SelectContent></Select>
                : property.type === 'object' || property.type === 'array' ? <Textarea id={id} value={typeof value === 'string' ? value : ''} onChange={event => setValue(event.target.value)} className="min-h-24 font-mono text-[13px]" placeholder={property.type === 'array' ? 'Comma-separated values or JSON array' : '{ }'} spellCheck={false} />
                : <Input id={id} type={property.type === 'integer' || property.type === 'number' ? 'number' : 'text'} step={property.type === 'integer' ? 1 : property.type === 'number' ? 'any' : undefined} min={property.minimum} max={property.maximum} minLength={property.minLength} maxLength={property.maxLength} pattern={property.pattern} value={typeof value === 'string' ? value : ''} onChange={event => setValue(event.target.value)} />}
              {property.description ? <p className="text-xs leading-5 text-aurora-text-subtle">{property.description}</p> : null}
            </div>
          })}</div>
          {selected && Object.keys(selected.inputSchema.properties ?? {}).length === 0 ? <p className="rounded-aurora-2 border border-aurora-border-subtle bg-aurora-control-surface px-4 py-3 text-sm text-aurora-text-muted">This operation does not require parameters.</p> : null}
          {selected && isDestructiveOperation(selected.annotations) ? <label className="flex items-start gap-3 rounded-aurora-2 border border-destructive/40 bg-destructive/10 p-4 text-sm" htmlFor="depot-destructive-confirm"><Checkbox id="depot-destructive-confirm" checked={confirmed} onCheckedChange={checked => setConfirmed(checked === true)} /><span><strong className="block text-aurora-text-primary">Confirm permanent operation</strong><span className="mt-1 block text-aurora-text-muted">I understand this action can remove or irreversibly change Depot data.</span></span></label> : null}
          {result !== null && result.operation === selected?.name ? <div aria-live="polite"><p className={AURORA_MUTED_LABEL}>Result</p><pre className="mt-2 max-h-72 overflow-auto rounded-aurora-2 border border-aurora-border-subtle bg-aurora-control-surface p-4 text-xs leading-5 text-aurora-text-muted">{JSON.stringify(result.value, null, 2)}</pre></div> : null}</div>
        <DialogFooter><Button variant="outline" onClick={close}>Close</Button><Button onClick={() => void execute()} disabled={running || Boolean(selected && isDestructiveOperation(selected.annotations) && !confirmed)}>{running ? <Loader2 className="size-4 animate-spin" /> : <Play className="size-4" />}{isDestructiveOperation(selected?.annotations) ? 'Run destructive operation' : selected?.annotations?.readOnlyHint === true ? 'Run operation' : 'Review and run'}</Button></DialogFooter>
      </DialogContent>
    </Dialog>
  </>
}

export function DepotAdministrationPage() {
  const [workspace, setWorkspace] = useState<Workspace>('overview')
  const [status, setStatus] = useState<DepotStatus | null>(null)
  const [operations, setOperations] = useState<DepotOperation[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const loadGeneration = useRef(0)
  const loadController = useRef<AbortController | null>(null)

  const load = useCallback(async () => {
    const generation = ++loadGeneration.current
    loadController.current?.abort()
    const controller = new AbortController()
    loadController.current = controller
    setLoading(true); setError(null)
    try {
      const [nextStatus, nextOperations] = await Promise.all([depotStatus(controller.signal), depotOperations(controller.signal)])
      if (generation !== loadGeneration.current) return
      setStatus(nextStatus); setOperations(nextOperations)
    } catch (cause) {
      if (controller.signal.aborted || generation !== loadGeneration.current) return
      setStatus(null)
      setOperations([])
      setError(getErrorMessage(cause, 'Unable to load Depot administration.'))
    }
    finally { if (generation === loadGeneration.current) setLoading(false) }
  }, [])
  useEffect(() => { void load(); return () => loadController.current?.abort() }, [load])

  const counts = useMemo(() => ({
    catalog: operations.filter(operation => operationWorkspace(operation) === 'catalog').length,
    access: operations.filter(operation => operationWorkspace(operation) === 'access').length,
    operations: operations.filter(operation => operationWorkspace(operation) === 'operations').length,
  }), [operations])
  const authority = !status?.enabled ? 'offline' : status.authority ?? (operations.length === 0 ? 'unknown' : operations.some(operation => operation.annotations?.readOnlyHint === false) ? 'write' : 'read')

  return <><AppHeader breadcrumbs={[{ label: 'Depot', href: '/depot/' }, { label: 'Administration' }]} /><div className={cn(AURORA_PAGE_FRAME, 'gap-3.5')}>
    <ConsoleHero eyebrow="Depot · Control room" title="Administration" description="Operate every capability published by the selected Depot authority through Labby’s authenticated control plane." pulse={{ color: status?.enabled ? 'var(--aurora-success)' : 'var(--aurora-warn)', label: status?.enabled ? 'Authority connected' : 'Authority unavailable' }} actions={<div className="flex gap-[7px]"><Button variant="outline" size="sm" className="h-9 gap-[7px] rounded-[10px] px-3.5 text-[12.5px] font-[650]" data-visible-label="1" asChild><a href="/settings/depot/"><Database className="size-[13px]" />Authorities</a></Button><Button variant="outline" size="sm" className="h-9 gap-[7px] rounded-[10px] px-3.5 text-[12.5px] font-[650]" data-visible-label="1" onClick={() => void load()} disabled={loading}>{loading ? <Loader2 className="size-[13px] animate-spin" /> : <RefreshCw className="size-[13px]" />}Refresh</Button></div>} stats={[
      { label: 'Canonical operations', value: loading || error ? '—' : operations.length },
      { label: 'Catalog', value: loading || error ? '—' : counts.catalog, tone: 'var(--aurora-accent-strong)' },
      { label: 'Access', value: loading || error ? '—' : counts.access, tone: 'var(--aurora-warn)' },
      { label: 'Operations', value: loading || error ? '—' : counts.operations, tone: 'var(--aurora-success)' },
      { label: 'Authority', value: authority },
    ]} footer={<nav aria-label="Depot administration workspaces" className="aurora-scrollbar flex gap-0.5 overflow-x-auto rounded-b-aurora-3 border-t border-aurora-border-default bg-aurora-control-surface px-5">{WORKSPACES.map(({ id, label, icon: Icon }) => <button key={id} type="button" aria-current={workspace === id ? 'page' : undefined} onClick={() => setWorkspace(id)} className="flex h-[38px] shrink-0 items-center gap-2 border-b-2 border-transparent px-3.5 text-[12.5px] font-[650] text-aurora-text-muted transition-colors hover:text-aurora-text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-aurora-accent-primary aria-[current=page]:border-aurora-accent-primary aria-[current=page]:text-aurora-text-primary"><Icon className="size-[13px]" />{label}{id !== 'overview' ? <span className={cn('inline-flex h-[19px] min-w-5 items-center justify-center rounded-[5px] border px-[5px] text-[10.5px] font-bold tabular-nums', workspace === id ? 'border-aurora-accent-primary bg-aurora-selected-bg text-aurora-accent-strong' : 'border-aurora-border-default bg-aurora-page-bg text-aurora-text-muted')}>{counts[id]}</span> : null}</button>)}</nav>} />
    {error ? <DashboardPanel title="Depot unavailable"><p className="text-sm text-destructive">{error}</p><Button className="mt-3" variant="outline" size="sm" onClick={() => void load()}>Retry</Button></DashboardPanel> : null}
    {!error && workspace === 'overview' ? <AdministrationOverview onOpen={setWorkspace} /> : null}
    {!error && workspace !== 'overview' ? <OperationGrid key={workspace} operations={operations} workspace={workspace} /> : null}
  </div></>
}
