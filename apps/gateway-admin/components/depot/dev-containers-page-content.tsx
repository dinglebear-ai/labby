'use client'

import { useCallback, useEffect, useState } from 'react'
import { CirclePlus, Container, Cpu, HardDrive, MemoryStick, Play, RefreshCw, Square, Terminal, Trash2, Settings2 } from 'lucide-react'
import { ActionConfirmationDialog } from '@/components/action-confirmation-dialog'
import { ConsoleHero } from '@/components/console/console-hero'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { isAbortError } from '@/lib/api/service-action-client'
import { authorityIdentity, useBrowserSession } from '@/lib/auth/session'
import * as api from '@/lib/dev-containers/client'
import type { DevContainer } from '@/lib/dev-containers/client'

export function ContainerCard({ item, busy, canOperate, canDelete, onOperate, onDestroy }: {
  item: DevContainer; busy: boolean; canOperate: boolean; canDelete: boolean
  onOperate: (operation: 'start' | 'stop' | 'reconcile') => void; onDestroy: () => void
}) {
  const running = item.observed_state === 'running'
  const tone = running ? 'var(--aurora-success)' : item.observed_state === 'stopped' ? 'var(--aurora-text-muted)' : 'var(--aurora-warn)'
  return <article className="min-w-0 overflow-hidden rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-strong shadow-[var(--aurora-shadow-medium)]">
    <header className="flex items-center gap-2 border-b border-aurora-border-subtle bg-aurora-control-surface px-[15px] py-2.5">
      <Container aria-hidden="true" className="size-4 shrink-0 text-aurora-success" />
      <h2 className="min-w-0 flex-1 truncate font-display text-sm font-bold text-aurora-text-primary">{item.instance_id}</h2>
      <span title={`Observed state: ${item.observed_state}`} className="grid size-[22px] shrink-0 place-items-center rounded-[7px] border" style={{ color: tone, borderColor: `color-mix(in srgb, ${tone} 32%, transparent)`, background: `color-mix(in srgb, ${tone} 10%, transparent)` }}><span className="size-1.5 rounded-full bg-current" /><span className="sr-only">{item.observed_state}</span></span>
    </header>
    <div className="flex flex-col gap-2.5 px-[13px] py-3">
      <p className="truncate text-[11.5px] text-aurora-text-muted">{item.owner_kind} · {item.owner_id}</p>
      <div className="flex flex-col gap-1.5">{[[Cpu, 'CPU'], [MemoryStick, 'Memory'], [HardDrive, 'Storage']].map(([Icon, label]) => { const Glyph = Icon as typeof Cpu; return <div key={String(label)} className="flex items-center gap-2" title={`${label} measurements are not reported by this server`}><Glyph aria-hidden="true" className="size-[11px] shrink-0 text-aurora-text-muted" /><span className="h-[5px] min-w-0 flex-1 rounded-full bg-aurora-page-bg" /><span className="w-[90px] shrink-0 text-right text-[10px] text-aurora-text-muted">Not reported</span></div> })}</div>
      <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-[10.5px]"><dt className="text-aurora-text-muted">Desired state</dt><dd className="truncate text-right text-aurora-text-primary">{item.desired_state}</dd><dt className="text-aurora-text-muted">Observed state</dt><dd className="truncate text-right text-aurora-text-primary">{item.observed_state}</dd></dl>
      <p className="text-[10px] text-aurora-text-muted">Image, toolchain and mount details are not reported.</p>
      <footer className="flex items-center gap-1 border-t border-aurora-border-subtle pt-[9px]">
        <span className="mr-auto text-[10px] text-aurora-text-muted">Container controls</span>
        <span title="Environment editing is not supported by this server"><Button disabled size="icon-sm" variant="ghost" aria-label={`Environment unavailable for ${item.instance_id}`}><Settings2 /></Button></span>
        <span title="Container shell is not supported by this server"><Button disabled size="icon-sm" variant="ghost" aria-label={`Shell unavailable for ${item.instance_id}`}><Terminal /></Button></span>
        {canOperate ? <><Button size="icon-sm" variant="ghost" title={running ? 'Stop container' : 'Start container'} aria-label={`${running ? 'Stop' : 'Start'} ${item.instance_id}`} disabled={busy} onClick={() => onOperate(running ? 'stop' : 'start')}>{running ? <Square /> : <Play />}</Button><Button size="icon-sm" variant="ghost" title="Reconcile container" aria-label={`Reconcile ${item.instance_id}`} disabled={busy} onClick={() => onOperate('reconcile')}><RefreshCw /></Button></> : null}
        {canDelete ? <Button size="icon-sm" variant="ghost" title="Destroy container" aria-label={`Destroy ${item.instance_id}`} disabled={busy} onClick={onDestroy}><Trash2 /></Button> : null}
      </footer>
    </div>
  </article>
}

export function DevContainersPageContent() {
  const session = useBrowserSession()
  const authority = session.status === 'authenticated' ? session.authority : undefined
  const workspaceIdentity = authorityIdentity(authority)
  return (
    <DevContainersWorkspace
      key={workspaceIdentity}
      workspaceIdentity={workspaceIdentity}
      ownerKind={authority?.activeOwner.kind}
      capabilityList={authority?.capabilities ?? []}
    />
  )
}

function DevContainersWorkspace({ workspaceIdentity, ownerKind, capabilityList }: {
  workspaceIdentity: string
  ownerKind?: DevContainer['owner_kind']
  capabilityList: readonly string[]
}) {
  const capabilities = new Set(capabilityList)
  const [instances, setInstances] = useState<DevContainer[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string>()
  const [creating, setCreating] = useState(false)
  const [instanceId, setInstanceId] = useState('')
  const [templateId, setTemplateId] = useState('')
  const [busy, setBusy] = useState<string>()
  const [destroyTarget, setDestroyTarget] = useState<DevContainer>()
  const load = useCallback(async (signal?: AbortSignal) => {
    setLoading(true); setError(undefined); setInstances([])
    try { setInstances(await api.listDevContainers(signal)) }
    catch (reason) { if (!isAbortError(reason)) setError(reason instanceof Error ? reason.message : 'Dev Containers are unavailable.') }
    finally { if (!signal?.aborted) setLoading(false) }
  }, [])
  useEffect(() => { const controller = new AbortController(); void load(controller.signal); return () => controller.abort() }, [load, workspaceIdentity])
  const operate = async (item: DevContainer, operation: 'start' | 'stop' | 'destroy' | 'reconcile') => {
    setBusy(item.instance_id); setError(undefined)
    try { await api.operateDevContainer(item.instance_id, operation); setDestroyTarget(undefined); await load() }
    catch (reason) { if (!isAbortError(reason)) setError(reason instanceof Error ? reason.message : 'The operation failed.') }
    finally { setBusy(undefined) }
  }
  const create = async () => {
    if (!instanceId.trim() || !templateId.trim()) return
    setBusy('create'); setError(undefined)
    try { await api.createDevContainer(instanceId.trim(), templateId.trim()); setInstanceId(''); setTemplateId(''); setCreating(false); await load() }
    catch (reason) { if (!isAbortError(reason)) setError(reason instanceof Error ? reason.message : 'Container creation failed.') }
    finally { setBusy(undefined) }
  }
  const count = (state: string) => instances.filter(item => item.observed_state === state).length
  return <>
    <ConsoleHero variant="authoring" iconTone="success" icon={<Container className="size-[22px] text-aurora-success" />} eyebrow={`Workspace · ${ownerKind ?? 'unavailable'}`} title="Dev Containers" description="Containers created from administrator-approved templates. Manage their desired state and inspect the runtime state reported by this workspace." pulse={{ color: error ? 'var(--aurora-warn)' : loading ? 'var(--aurora-text-muted)' : 'var(--aurora-success)', label: loading ? 'loading' : error ? 'inventory unavailable' : `${count('running')} running` }} actions={<><Button size="icon-sm" variant="ghost" title="Refresh containers" aria-label="Refresh containers" onClick={() => void load()} disabled={loading}><RefreshCw /></Button><span title={capabilities.has('scope.create') ? undefined : 'This workspace does not grant container creation'}><Button size="icon" variant="outline" aria-label="New container" title="New container" disabled={!capabilities.has('scope.create')} onClick={() => setCreating(true)}><CirclePlus className="size-[15px]" /></Button></span></>} stats={[{ label: 'Containers', value: loading || error ? '—' : instances.length, suffix: 'visible' }, { label: 'Running', value: loading || error ? '—' : count('running'), tone: 'var(--aurora-success)' }, { label: 'Stopped', value: loading || error ? '—' : count('stopped') }, { label: 'Pending', value: loading || error ? '—' : count('pending'), tone: 'var(--aurora-accent-strong)' }]} />
    {error ? <div role="alert" className="rounded-aurora-2 border border-aurora-error/35 bg-aurora-error/5 p-3 text-xs text-aurora-error">{error}</div> : null}
    {loading ? <p role="status" className="py-10 text-center text-xs text-aurora-text-muted">Loading container inventory…</p> : !instances.length ? <div className="rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-strong px-5 py-10 text-center"><p className="font-display text-[15px] font-bold text-aurora-text-primary">{error ? 'Container inventory unavailable.' : 'No containers in this workspace.'}</p><p className="mt-1 text-xs text-aurora-text-muted">Create a container from an approved template to get started.</p></div> : <div className="grid items-start gap-3 [grid-template-columns:repeat(auto-fill,minmax(min(100%,310px),1fr))]">{instances.map(item => <ContainerCard key={item.instance_id} item={item} busy={busy === item.instance_id} canOperate={capabilities.has('scope.operate')} canDelete={capabilities.has('scope.delete')} onOperate={operation => void operate(item, operation)} onDestroy={() => setDestroyTarget(item)} />)}</div>}
    <Dialog open={creating} onOpenChange={setCreating}><DialogContent><DialogHeader><DialogTitle>New Container</DialogTitle><DialogDescription>Create from an administrator-approved template. Template discovery is not available on this server; enter its exact identifier.</DialogDescription></DialogHeader><label className="text-xs text-aurora-text-muted">Container ID<Input className="mt-1.5" value={instanceId} onChange={event => setInstanceId(event.target.value)} /></label><label className="text-xs text-aurora-text-muted">Approved template ID<Input className="mt-1.5" value={templateId} onChange={event => setTemplateId(event.target.value)} /></label>{error ? <p role="alert" className="text-xs text-aurora-error">{error}</p> : null}<Button onClick={() => void create()} disabled={busy === 'create' || !instanceId.trim() || !templateId.trim()}>Create container</Button></DialogContent></Dialog>
    <ActionConfirmationDialog open={Boolean(destroyTarget)} onOpenChange={open => { if (!open) setDestroyTarget(undefined) }} title="Destroy this container?" description={`This permanently destroys ${destroyTarget?.instance_id ?? 'the selected container'} and cannot be undone.`} confirmLabel="Destroy container" busy={busy === destroyTarget?.instance_id} onConfirm={() => destroyTarget ? void operate(destroyTarget, 'destroy') : undefined} />
  </>
}
