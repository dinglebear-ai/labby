'use client'

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Activity, Boxes, Database, KeyRound, Loader2, Play, RefreshCw, Search, ShieldCheck, Wrench } from 'lucide-react'
import { toast } from 'sonner'

import { AppHeader } from '@/components/app-header'
import { AURORA_BADGE_LABEL, AURORA_CARD_TITLE, AURORA_DENSE_META, AURORA_MUTED_LABEL, AURORA_PAGE_FRAME } from '@/components/aurora/tokens'
import { ConsoleHero } from '@/components/console/console-hero'
import { DashboardPanel } from '@/components/dashboard/panel'
import { ArtifactControlPlane } from '@/components/skills/artifact-control-plane'
import { DepotProvidersPage } from '@/components/settings/depot-providers-page'
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

function OperationGrid({ operations, workspace }: { operations: DepotOperation[]; workspace: Exclude<Workspace, 'overview'> }) {
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
    <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
      <label className="relative min-w-64 flex-1" htmlFor={`depot-operation-search-${workspace}`}><Search aria-hidden="true" className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-aurora-text-subtle"/><span className="sr-only">Search {workspace} operations</span><Input id={`depot-operation-search-${workspace}`} value={query} onChange={event => { setQuery(event.target.value); setLimit(48) }} className="pl-9" placeholder="Search operations" /></label>
      <span className={AURORA_DENSE_META}>{matching.length} operations</span>
    </div>
    <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
      {visible.map(operation => {
        const readOnly = operation.annotations?.readOnlyHint === true
        const destructive = isDestructiveOperation(operation.annotations)
        return <button key={operation.name} type="button" onClick={() => open(operation)} className="group rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-medium p-4 text-left shadow-[var(--aurora-shadow-subtle)] transition-[border-color,background-color,transform] duration-150 hover:-translate-y-0.5 hover:border-aurora-accent-primary/40 hover:bg-aurora-hover-bg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-focus-ring">
          <div className="flex items-start justify-between gap-3"><span className={AURORA_CARD_TITLE}>{operation.title}</span><Badge variant="outline" className={destructive ? 'text-destructive' : readOnly ? 'text-aurora-success' : 'text-aurora-warn'}>{destructive ? 'Destructive' : readOnly ? 'Read' : 'Admin'}</Badge></div>
          <p className="mt-2 text-sm leading-[1.55] text-aurora-text-muted">{operation.description}</p>
          <code className={cn(AURORA_DENSE_META, 'mt-3 block truncate text-aurora-accent-primary')}>{operation.name}</code>
        </button>
      })}
    </div>
    {visible.length < matching.length ? <div className="mt-4 flex justify-center"><Button variant="outline" onClick={() => setLimit(current => Math.min(current + 48, matching.length))}>Show more ({matching.length - visible.length} remaining)</Button></div> : null}
    {matching.length === 0 ? <p className="rounded-aurora-2 border border-aurora-border-subtle p-6 text-center text-sm text-aurora-text-muted">No operations match this search.</p> : null}
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

  return <><AppHeader breadcrumbs={[{ label: 'Depot', href: '/depot/' }, { label: 'Administration' }]} /><div className={AURORA_PAGE_FRAME}>
    <ConsoleHero eyebrow="Depot · Control room" title="Administration" description="Operate every capability published by the selected Depot authority through Labby’s authenticated control plane." pulse={{ color: status?.enabled ? 'var(--aurora-success)' : 'var(--aurora-warn)', label: status?.enabled ? 'Authority connected' : 'Authority unavailable' }} actions={<div className="flex gap-2"><Button variant="outline" size="sm" asChild><a href="/settings/depot/"><Database className="size-4" />Authorities</a></Button><Button variant="outline" size="sm" onClick={() => void load()} disabled={loading}>{loading ? <Loader2 className="size-4 animate-spin" /> : <RefreshCw className="size-4" />}Refresh</Button></div>} stats={[
      { label: 'Canonical operations', value: operations.length || '—', icon: <Activity size={12}/> },
      { label: 'Catalog', value: counts.catalog || '—', icon: <Boxes size={12}/>, tone: 'var(--aurora-accent-strong)' },
      { label: 'Access', value: counts.access || '—', icon: <KeyRound size={12}/>, tone: 'var(--aurora-warn)' },
      { label: 'Operations', value: counts.operations || '—', icon: <Wrench size={12}/>, tone: 'var(--aurora-success)' },
      { label: 'Authority', value: authority, icon: <ShieldCheck size={12}/> },
    ]} />
    <nav aria-label="Depot administration workspaces" className="flex overflow-x-auto border-b border-aurora-border-subtle px-1 sm:px-3">{WORKSPACES.map(({ id, label, icon: Icon }) => <button key={id} type="button" aria-current={workspace === id ? 'page' : undefined} onClick={() => setWorkspace(id)} className="flex shrink-0 items-center gap-2 border-b-2 border-transparent px-4 py-3 text-sm font-semibold text-aurora-text-muted transition-colors hover:text-aurora-text-primary aria-[current=page]:border-aurora-accent-primary aria-[current=page]:text-aurora-text-primary"><Icon className="size-4" />{label}{id !== 'overview' ? <span className={cn(AURORA_BADGE_LABEL, 'opacity-70')}>{counts[id]}</span> : null}</button>)}</nav>
    {error ? <DashboardPanel title="Depot unavailable"><p className="text-sm text-destructive">{error}</p><Button className="mt-3" variant="outline" size="sm" onClick={() => void load()}>Retry</Button></DashboardPanel> : null}
    {!error && workspace === 'overview' ? <div className="grid gap-4 lg:grid-cols-3"><DashboardPanel title="Catalog lifecycle" icon={<Boxes className="size-4" />}><p className="text-sm leading-6 text-aurora-text-muted">Discovery, canonical artifacts, sources, durable jobs, uploads, bundles, and publication share one workspace.</p><Button className="mt-4" size="sm" onClick={() => setWorkspace('catalog')}>Open Catalog</Button></DashboardPanel><DashboardPanel title="Access & governance" icon={<ShieldCheck className="size-4" />}><p className="text-sm leading-6 text-aurora-text-muted">Token administration and publication policy use Depot’s canonical schemas and Labby’s admin guard.</p><Button className="mt-4" size="sm" variant="outline" onClick={() => setWorkspace('access')}>Open Access</Button></DashboardPanel><DashboardPanel title="System operations" icon={<Wrench className="size-4" />}><p className="text-sm leading-6 text-aurora-text-muted">Status, CAS audits, maintenance, and migrations remain explicit, reviewable operations.</p><Button className="mt-4" size="sm" variant="outline" onClick={() => setWorkspace('operations')}>Open Operations</Button></DashboardPanel></div> : null}
    {!error && workspace === 'catalog' ? <div className="space-y-5"><ArtifactControlPlane /><DashboardPanel title="Canonical catalog operations" icon={<Boxes className="size-4" />}><p className="mb-4 text-sm text-aurora-text-muted">Direct access to every catalog operation advertised by this Depot authority.</p><OperationGrid operations={operations} workspace="catalog" /></DashboardPanel></div> : null}
    {!error && workspace === 'access' ? <OperationGrid operations={operations} workspace="access" /> : null}
    {!error && workspace === 'operations' ? <div className="space-y-5"><DepotProvidersPage /><DashboardPanel title="Depot maintenance" icon={<Wrench className="size-4" />}><OperationGrid operations={operations} workspace="operations" /></DashboardPanel></div> : null}
  </div></>
}
