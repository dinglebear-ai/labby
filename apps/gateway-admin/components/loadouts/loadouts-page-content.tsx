'use client'

import { useMemo, useState } from 'react'
import Link from 'next/link'
import { BookOpen, Boxes, Cable, Loader2, Pencil, Plus, ShieldCheck, Trash2, Wrench } from 'lucide-react'
import { toast } from 'sonner'
import { ActionConfirmationDialog } from '@/components/action-confirmation-dialog'
import { AppHeader } from '@/components/app-header'
import { LibraryTabs } from '@/components/depot/depot-workspace-pages'
import { ConsoleHero, type ConsoleHeroStat } from '@/components/console/console-hero'
import { DashboardPanel } from '@/components/dashboard/panel'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { AURORA_CARD_TITLE, AURORA_DENSE_META, AURORA_MUTED_LABEL, AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { useGatewaySnapshots, useGatewayMutations, useLoadouts, useProtectedMcpRoutes, useSupportedServices } from '@/lib/hooks/use-gateways'
import type { GatewayLoadout } from '@/lib/types/gateway'
import { cn, getErrorMessage } from '@/lib/utils'
import { LOADOUT_CAPABILITIES, LoadoutFormDialog } from './loadout-form-dialog'

export function LoadoutsPageContent() {
  const [formOpen, setFormOpen] = useState(false)
  const [editing, setEditing] = useState<GatewayLoadout | null>(null)
  const [deleting, setDeleting] = useState<GatewayLoadout | null>(null)
  const [deleteBusy, setDeleteBusy] = useState(false)
  const { data: loadouts = [], isLoading, error } = useLoadouts()
  // Gateway configuration is only needed to populate the add/edit dialog. A full
  // gateway list can cold-connect many stdio upstreams, so do not hydrate the
  // fleet merely to render the Loadouts overview.
  const {
    data: gateways = [],
    isLoading: gatewaysLoading,
    error: gatewaysError,
  } = useGatewaySnapshots(formOpen)
  const { data: services = [] } = useSupportedServices()
  const {
    data: protectedRoutes = [],
    isLoading: protectedRoutesLoading,
    error: protectedRoutesError,
  } = useProtectedMcpRoutes()
  const { addLoadout, patchLoadout, removeLoadout, stageLoadoutUpdate, stageLoadoutRemove } = useGatewayMutations()

  const gatewayOptions = useMemo(() => gateways.filter(g => g.source !== 'in_process' && g.transport !== 'in_process').map(g => ({ value: g.name, label: g.name, meta: g.config.url ?? g.config.command ?? g.transport })), [gateways])
  const serviceOptions = useMemo(() => services.map(s => ({ value: s.key, label: s.display_name, meta: s.description })), [services])
  const mountedBy = useMemo(() => {
    const map = new Map<string, string[]>()
    for (const route of protectedRoutes) {
      const name = route.target?.kind === 'gateway_subset' ? route.target.loadout : undefined
      if (name) map.set(name, [...(map.get(name) ?? []), route.name])
    }
    return map
  }, [protectedRoutes])
  const pendingRestartCount = loadouts.filter((loadout) => loadout.restart_required).length
  const stats: ConsoleHeroStat[] = [
    { label: 'Loadouts', value: isLoading ? '—' : loadouts.length, icon: <Boxes size={12} /> },
    { label: 'Upstream refs', value: isLoading ? '—' : loadouts.reduce((n, x) => n + x.upstreams.length, 0), icon: <Cable size={12} /> },
    { label: 'Service refs', value: isLoading ? '—' : loadouts.reduce((n, x) => n + x.services.length, 0), icon: <Wrench size={12} /> },
    { label: 'Skills enabled', value: isLoading ? '—' : loadouts.filter(x => x.expose_skills).length, icon: <BookOpen size={12} /> },
  ]

  return <>
    <AppHeader breadcrumbs={[{ label: 'Depot' }, { label: 'Library' }, { label: 'Loadouts' }]} />
    <div className={cn(AURORA_PAGE_SHELL, 'flex-1')}><div className={AURORA_PAGE_FRAME}>
      <LibraryTabs active="loadouts" />
      <ConsoleHero eyebrow="Control Plane" pulse={loadouts.length ? { color: 'var(--aurora-success)', label: loadouts.length + ' configured' } : undefined} title="Loadouts" stats={stats} actions={<Button size="sm" onClick={() => { setEditing(null); setFormOpen(true) }}><Plus className="size-4" />Add Loadout</Button>} />
      <DashboardPanel title="Reusable capability projections" icon={<ShieldCheck className="size-4" />} action={<Button variant="outline" size="sm" asChild><Link href="/gateway">Mount on a route</Link></Button>}><p className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>Loadouts define reusable projections; they do not create an endpoint by themselves. Mount one on an enabled protected MCP route to make it callable. Per-upstream exposure policies remain enforced underneath.</p></DashboardPanel>
      {protectedRoutesError && <div role="alert" className="rounded-lg border border-destructive/35 bg-destructive/10 px-3 py-2 text-sm text-aurora-text-primary">Could not verify protected route mounts. Editing and removal are disabled to prevent applying the wrong update mode. {getErrorMessage(protectedRoutesError, 'Protected routes failed to load')}</div>}
      {pendingRestartCount > 0 && <div className="rounded-lg border border-aurora-warning/35 bg-aurora-warning/10 px-3 py-2 text-sm text-aurora-text-primary">{pendingRestartCount} Loadout change{pendingRestartCount === 1 ? ' is' : 's are'} saved for restart. Running protected routes still use their startup projections.</div>}
      {isLoading ? <DashboardPanel title="Loadouts" icon={<Loader2 className="size-4 animate-spin" />}><span className={AURORA_MUTED_LABEL}>Loading Loadouts…</span></DashboardPanel>
      : error ? <DashboardPanel title="Loadouts" icon={<ShieldCheck className="size-4 text-destructive" />}><span className={AURORA_CARD_TITLE}>Could not load Loadouts</span><p className={AURORA_DENSE_META}>{getErrorMessage(error, 'Gateway Loadout request failed')}</p></DashboardPanel>
      : loadouts.length === 0 ? <DashboardPanel title="Loadouts" icon={<Boxes className="size-4" />}><span className={AURORA_CARD_TITLE}>No Loadouts configured</span><p className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>Add a Loadout to create a reusable gateway capability projection for protected MCP routes.</p></DashboardPanel>
      : <div className="grid gap-3 xl:grid-cols-2">{loadouts.map(loadout => {
        const mounts = mountedBy.get(loadout.name) ?? []
        const caps = LOADOUT_CAPABILITIES.filter(([key]) => loadout[key])
        const routeStateUnavailable = protectedRoutesLoading || Boolean(protectedRoutesError)
        return <DashboardPanel key={loadout.name} title={loadout.name} icon={<Boxes className="size-4" />} meta={loadout.restart_required ? 'restart required' : mounts.length ? mounts.length + ' route' + (mounts.length === 1 ? '' : 's') : protectedRoutesLoading ? 'checking mounts' : 'unmounted'} action={<div className="flex gap-1"><Button variant="ghost" size="icon-sm" aria-label={'Edit ' + loadout.name} disabled={routeStateUnavailable || loadout.pending_operation === 'remove'} onClick={() => { setEditing(loadout); setFormOpen(true) }}><Pencil className="size-3.5" /></Button><Button variant="ghost" size="icon-sm" aria-label={'Remove ' + loadout.name} disabled={routeStateUnavailable || loadout.pending_operation === 'remove'} onClick={() => setDeleting(loadout)}><Trash2 className="size-3.5" /></Button></div>}>
          {loadout.description && <p className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>{loadout.description}</p>}
          <div className="flex flex-wrap gap-2">{loadout.restart_required && <Badge variant="outline" className="border-aurora-warning/50 text-aurora-warning">Restart · {loadout.pending_operation ?? 'update'}</Badge>}{caps.map(([key, label, , Icon]) => <Badge key={key} variant="secondary" className="gap-1"><Icon className="size-3" />{label}</Badge>)}</div>
          <div className="grid gap-3 sm:grid-cols-2"><div><p className={AURORA_MUTED_LABEL}>Upstreams</p><div className="mt-1 flex flex-wrap gap-1.5">{loadout.upstreams.length ? loadout.upstreams.map(name => <Badge key={name} variant="outline">{name}</Badge>) : <span className={AURORA_DENSE_META}>None</span>}</div></div><div><p className={AURORA_MUTED_LABEL}>Lab services</p><div className="mt-1 flex flex-wrap gap-1.5">{loadout.services.length ? loadout.services.map(name => <Badge key={name} variant="outline">{name}</Badge>) : <span className={AURORA_DENSE_META}>None</span>}</div></div></div>
          {mounts.length > 0 && <div><p className={AURORA_MUTED_LABEL}>Mounted by</p><div className="mt-1 flex flex-wrap gap-1.5">{mounts.map(name => <Badge key={name} variant="outline">{name}</Badge>)}</div></div>}
        </DashboardPanel>
      })}</div>}
    </div></div>
    <LoadoutFormDialog open={formOpen} loadout={editing} gatewayOptions={gatewayOptions} gatewayOptionsLoading={gatewaysLoading} gatewayOptionsError={gatewaysError ? getErrorMessage(gatewaysError, 'Gateway options failed to load') : null} serviceOptions={serviceOptions} onOpenChange={setFormOpen} onSave={async (original, draft) => { if (original) { const current = loadouts.find((loadout) => loadout.name === original); const mounted = (mountedBy.get(original)?.length ?? 0) > 0; if (mounted || current?.restart_required) { await stageLoadoutUpdate(original, draft); toast.success('Loadout ' + draft.name + ' saved. Restart Labby to apply it to mounted routes.') } else { await patchLoadout(original, draft); toast.success('Loadout ' + draft.name + ' updated.') } } else { await addLoadout(draft); toast.success('Loadout ' + draft.name + ' added.') } }} />
    <ActionConfirmationDialog open={deleting !== null} title="Remove Loadout?" description={deleting ? 'Remove ' + deleting.name + '? If a running protected route still uses it, first stage that route away from this Loadout; the Loadout removal can then be staged for the same restart.' : ''} confirmLabel="Remove Loadout" busy={deleteBusy} onOpenChange={open => !open && setDeleting(null)} onConfirm={async () => { if (!deleting) return; setDeleteBusy(true); try { const mounted = (mountedBy.get(deleting.name)?.length ?? 0) > 0; if (mounted || deleting.restart_required) { await stageLoadoutRemove(deleting.name); toast.success('Loadout removal saved. Restart Labby to apply it.') } else { await removeLoadout(deleting.name); toast.success('Loadout ' + deleting.name + ' removed.') } setDeleting(null) } catch (e) { toast.error(getErrorMessage(e, 'Failed to remove Loadout')) } finally { setDeleteBusy(false) } }} />
  </>
}
