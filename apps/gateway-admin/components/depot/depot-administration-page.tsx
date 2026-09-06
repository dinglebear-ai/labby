'use client'

import { useCallback, useEffect, useMemo, useState } from 'react'
import { Activity, Boxes, Database, KeyRound, Loader2, Play, RefreshCw, ShieldCheck, Wrench } from 'lucide-react'
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

const READ_OPERATIONS = new Set([
  'depot.skills.search', 'depot.skills.load', 'depot.skills.read', 'depot.artifacts.list',
  'depot.artifacts.get', 'depot.artifacts.exact', 'depot.skills.list', 'depot.skills.get',
  'depot.skills.search_skills_sh', 'depot.skills.search_ard', 'depot.skills.search_marketplace',
  'depot.bundles.list', 'depot.bundles.get', 'depot.sources.list', 'depot.system.status',
  'depot.mcp_registry.list', 'depot.acp_registry.list', 'depot.ingest.list', 'depot.ingest.get',
])

function operationWorkspace(name: string): Exclude<Workspace, 'overview'> {
  if (name.startsWith('depot.tokens.') || name.includes('set_publication') || name.includes('set_license')) return 'access'
  if (name.startsWith('depot.maintenance.') || name === 'depot.system.status') return 'operations'
  return 'catalog'
}

function OperationGrid({ operations, workspace }: { operations: DepotOperation[]; workspace: Exclude<Workspace, 'overview'> }) {
  const [selected, setSelected] = useState<DepotOperation | null>(null)
  const [form, setForm] = useState<OperationFormState>({})
  const [confirmed, setConfirmed] = useState(false)
  const [running, setRunning] = useState(false)
  const [result, setResult] = useState<unknown>(null)
  const visible = operations.filter(operation => operationWorkspace(operation.name) === workspace)

  const open = (operation: DepotOperation) => {
    const properties = operation.inputSchema.properties ?? {}
    setForm(initialOperationForm(properties))
    setConfirmed(false)
    setResult(null)
    setSelected(operation)
  }

  const execute = async () => {
    if (!selected) return
    let parsed: Record<string, unknown>
    try { parsed = operationParams(selected.inputSchema.properties ?? {}, selected.inputSchema.required ?? [], form) }
    catch (error) { toast.error(getErrorMessage(error, 'Review the operation parameters.')); return }
    setRunning(true)
    try {
      const response = await depotCall<{ result: unknown }>(selected.name, parsed)
      setResult(response.result)
      toast.success(`${selected.title} completed`)
    } catch (error) {
      toast.error(getErrorMessage(error, `Unable to run ${selected.title}.`))
    } finally { setRunning(false) }
  }

  return <>
    <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
      {visible.map(operation => {
        const readOnly = READ_OPERATIONS.has(operation.name)
        const destructive = isDestructiveOperation(operation.annotations)
        return <button key={operation.name} type="button" onClick={() => open(operation)} className="group rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-medium p-4 text-left shadow-[var(--aurora-shadow-subtle)] transition-[border-color,background-color,transform] duration-150 hover:-translate-y-0.5 hover:border-aurora-accent-primary/40 hover:bg-aurora-hover-bg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-focus-ring">
          <div className="flex items-start justify-between gap-3"><span className={AURORA_CARD_TITLE}>{operation.title}</span><Badge variant="outline" className={destructive ? 'text-destructive' : readOnly ? 'text-aurora-success' : 'text-aurora-warn'}>{destructive ? 'Destructive' : readOnly ? 'Read' : 'Admin'}</Badge></div>
          <p className="mt-2 text-sm leading-[1.55] text-aurora-text-muted">{operation.description}</p>
          <code className={cn(AURORA_DENSE_META, 'mt-3 block truncate text-aurora-accent-primary')}>{operation.name}</code>
        </button>
      })}
    </div>
    <Dialog open={Boolean(selected)} onOpenChange={openState => !openState && setSelected(null)}>
      <DialogContent className="max-h-[min(780px,calc(100vh-2rem))] max-w-2xl overflow-y-auto border-aurora-border-strong bg-aurora-panel-medium">
        <DialogHeader><DialogTitle>{selected?.title}</DialogTitle><DialogDescription>{selected?.description}</DialogDescription></DialogHeader>
        <div className="grid gap-5"><div><p className={AURORA_MUTED_LABEL}>Canonical operation</p><code className="mt-1 block text-sm text-aurora-accent-primary">{selected?.name}</code></div>
          <div className="grid gap-4 sm:grid-cols-2">{Object.entries(selected?.inputSchema.properties ?? {}).map(([name, raw]) => {
            const property = raw as OperationProperty
            const required = selected?.inputSchema.required?.includes(name) ?? false
            const id = `depot-operation-${name}`
            const value = form[name]
            const setValue = (next: string | boolean) => setForm(current => ({ ...current, [name]: next }))
            return <div key={name} className={cn('grid content-start gap-1.5', property.type === 'object' || property.type === 'array' ? 'sm:col-span-2' : '')}>
              <label className="text-sm font-semibold text-aurora-text-primary" htmlFor={id}>{name}{required ? <span className="ml-1 text-aurora-warn">*</span> : null}</label>
              {property.type === 'boolean' ? <label className="flex min-h-9 items-center gap-2 rounded-md border border-input px-3 text-sm text-aurora-text-muted" htmlFor={id}><Checkbox id={id} checked={value === true} onCheckedChange={checked => setValue(checked === true)} />Enabled</label>
                : Array.isArray(property.enum) ? <Select value={typeof value === 'string' ? value : ''} onValueChange={setValue}><SelectTrigger id={id}><SelectValue placeholder="Select a value" /></SelectTrigger><SelectContent>{property.enum.map(option => <SelectItem key={String(option)} value={String(option)}>{String(option)}</SelectItem>)}</SelectContent></Select>
                : property.type === 'object' || property.type === 'array' ? <Textarea id={id} value={typeof value === 'string' ? value : ''} onChange={event => setValue(event.target.value)} className="min-h-24 font-mono text-[13px]" placeholder={property.type === 'array' ? 'Comma-separated values or JSON array' : '{ }'} spellCheck={false} />
                : <Input id={id} type={property.type === 'integer' || property.type === 'number' ? 'number' : 'text'} min={property.minimum} max={property.maximum} value={typeof value === 'string' ? value : ''} onChange={event => setValue(event.target.value)} />}
              {property.description ? <p className="text-xs leading-5 text-aurora-text-subtle">{property.description}</p> : null}
            </div>
          })}</div>
          {selected && Object.keys(selected.inputSchema.properties ?? {}).length === 0 ? <p className="rounded-aurora-2 border border-aurora-border-subtle bg-aurora-control-surface px-4 py-3 text-sm text-aurora-text-muted">This operation does not require parameters.</p> : null}
          {selected && isDestructiveOperation(selected.annotations) ? <label className="flex items-start gap-3 rounded-aurora-2 border border-destructive/40 bg-destructive/10 p-4 text-sm" htmlFor="depot-destructive-confirm"><Checkbox id="depot-destructive-confirm" checked={confirmed} onCheckedChange={checked => setConfirmed(checked === true)} /><span><strong className="block text-aurora-text-primary">Confirm permanent operation</strong><span className="mt-1 block text-aurora-text-muted">I understand this action can remove or irreversibly change Depot data.</span></span></label> : null}
          {result !== null ? <div><p className={AURORA_MUTED_LABEL}>Result</p><pre className="mt-2 max-h-72 overflow-auto rounded-aurora-2 border border-aurora-border-subtle bg-aurora-control-surface p-4 text-xs leading-5 text-aurora-text-muted">{JSON.stringify(result, null, 2)}</pre></div> : null}</div>
        <DialogFooter><Button variant="outline" onClick={() => setSelected(null)}>Close</Button><Button onClick={() => void execute()} disabled={running || Boolean(selected && isDestructiveOperation(selected.annotations) && !confirmed)}>{running ? <Loader2 className="size-4 animate-spin" /> : <Play className="size-4" />}{isDestructiveOperation(selected?.annotations) ? 'Run destructive operation' : READ_OPERATIONS.has(selected?.name ?? '') ? 'Run operation' : 'Review and run'}</Button></DialogFooter>
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

  const load = useCallback(async () => {
    setLoading(true); setError(null)
    try {
      const [nextStatus, nextOperations] = await Promise.all([depotStatus(), depotOperations()])
      setStatus(nextStatus); setOperations(nextOperations)
    } catch (cause) { setError(getErrorMessage(cause, 'Unable to load Depot administration.')) }
    finally { setLoading(false) }
  }, [])
  useEffect(() => { void load() }, [load])

  const counts = useMemo(() => ({
    catalog: operations.filter(operation => operationWorkspace(operation.name) === 'catalog').length,
    access: operations.filter(operation => operationWorkspace(operation.name) === 'access').length,
    operations: operations.filter(operation => operationWorkspace(operation.name) === 'operations').length,
  }), [operations])
  const hasWriteAuthority = operations.some(operation => !READ_OPERATIONS.has(operation.name))

  return <><AppHeader breadcrumbs={[{ label: 'Depot', href: '/depot/' }, { label: 'Administration' }]} /><div className={AURORA_PAGE_FRAME}>
    <ConsoleHero eyebrow="Depot · Control room" title="Administration" description="Operate every capability published by the selected Depot authority through Labby’s authenticated control plane." pulse={{ color: status?.enabled ? 'var(--aurora-success)' : 'var(--aurora-warn)', label: status?.enabled ? 'Authority connected' : 'Authority unavailable' }} actions={<div className="flex gap-2"><Button variant="outline" size="sm" asChild><a href="/settings/depot/"><Database className="size-4" />Authorities</a></Button><Button variant="outline" size="sm" onClick={() => void load()} disabled={loading}>{loading ? <Loader2 className="size-4 animate-spin" /> : <RefreshCw className="size-4" />}Refresh</Button></div>} stats={[
      { label: 'Canonical operations', value: operations.length || '—', icon: <Activity size={12}/> },
      { label: 'Catalog', value: counts.catalog || '—', icon: <Boxes size={12}/>, tone: 'var(--aurora-accent-strong)' },
      { label: 'Access', value: counts.access || '—', icon: <KeyRound size={12}/>, tone: 'var(--aurora-warn)' },
      { label: 'Operations', value: counts.operations || '—', icon: <Wrench size={12}/>, tone: 'var(--aurora-success)' },
      { label: 'Authority', value: status?.enabled ? hasWriteAuthority ? 'write' : 'read' : 'offline', icon: <ShieldCheck size={12}/> },
    ]} />
    <nav aria-label="Depot administration workspaces" className="flex overflow-x-auto border-b border-aurora-border-subtle px-1 sm:px-3">{WORKSPACES.map(({ id, label, icon: Icon }) => <button key={id} type="button" aria-current={workspace === id ? 'page' : undefined} onClick={() => setWorkspace(id)} className="flex shrink-0 items-center gap-2 border-b-2 border-transparent px-4 py-3 text-sm font-semibold text-aurora-text-muted transition-colors hover:text-aurora-text-primary aria-[current=page]:border-aurora-accent-primary aria-[current=page]:text-aurora-text-primary"><Icon className="size-4" />{label}{id !== 'overview' ? <span className={cn(AURORA_BADGE_LABEL, 'opacity-70')}>{counts[id]}</span> : null}</button>)}</nav>
    {error ? <DashboardPanel title="Depot unavailable"><p className="text-sm text-destructive">{error}</p><Button className="mt-3" variant="outline" size="sm" onClick={() => void load()}>Retry</Button></DashboardPanel> : null}
    {workspace === 'overview' ? <div className="grid gap-4 lg:grid-cols-3"><DashboardPanel title="Catalog lifecycle" icon={<Boxes className="size-4" />}><p className="text-sm leading-6 text-aurora-text-muted">Discovery, canonical artifacts, sources, durable jobs, uploads, bundles, and publication share one workspace.</p><Button className="mt-4" size="sm" onClick={() => setWorkspace('catalog')}>Open Catalog</Button></DashboardPanel><DashboardPanel title="Access & governance" icon={<ShieldCheck className="size-4" />}><p className="text-sm leading-6 text-aurora-text-muted">Token administration and publication policy use Depot’s canonical schemas and Labby’s admin guard.</p><Button className="mt-4" size="sm" variant="outline" onClick={() => setWorkspace('access')}>Open Access</Button></DashboardPanel><DashboardPanel title="System operations" icon={<Wrench className="size-4" />}><p className="text-sm leading-6 text-aurora-text-muted">Status, CAS audits, maintenance, and migrations remain explicit, reviewable operations.</p><Button className="mt-4" size="sm" variant="outline" onClick={() => setWorkspace('operations')}>Open Operations</Button></DashboardPanel></div> : null}
    {workspace === 'catalog' ? <div className="space-y-5"><ArtifactControlPlane /><DashboardPanel title="Canonical catalog operations" icon={<Boxes className="size-4" />}><p className="mb-4 text-sm text-aurora-text-muted">Direct access to every catalog operation advertised by this Depot authority.</p><OperationGrid operations={operations} workspace="catalog" /></DashboardPanel></div> : null}
    {workspace === 'access' ? <OperationGrid operations={operations} workspace="access" /> : null}
    {workspace === 'operations' ? <div className="space-y-5"><DepotProvidersPage /><DashboardPanel title="Depot maintenance" icon={<Wrench className="size-4" />}><OperationGrid operations={operations} workspace="operations" /></DashboardPanel></div> : null}
  </div></>
}
