'use client'

import { useState } from 'react'
import dynamic from 'next/dynamic'
import { Layers } from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { useBrowserSession } from '@/lib/auth/session'
import { useGatewaySnapshots, useGatewayMutations, useSupportedServices } from '@/lib/hooks/use-gateways'
import { getErrorMessage } from '@/lib/utils'

const LoadoutFormDialog = dynamic(() => import('@/components/loadouts/loadout-form-dialog').then((module) => module.LoadoutFormDialog), { ssr: false })

export function LibraryNewLoadout() {
  const session = useBrowserSession()
  const canManage = session.status === 'authenticated' && Boolean(session.authority?.capabilities.includes('scope.manage'))
  const [open, setOpen] = useState(false)
  const { data: gateways = [], isLoading, error } = useGatewaySnapshots(open)
  const { data: services = [] } = useSupportedServices()
  const { addLoadout } = useGatewayMutations()
  return <>
    <Button data-visible-label variant="outline" disabled={!canManage} aria-label="New loadout" title={!canManage ? 'Loadout management requires scope management access' : 'New loadout'} onClick={() => setOpen(true)} className="rounded-[10px] font-[650] text-aurora-accent-strong" style={{ borderColor: 'color-mix(in srgb, var(--aurora-accent-primary) 55%, var(--aurora-border-strong))', background: 'color-mix(in srgb, var(--aurora-accent-primary) 9%, var(--aurora-panel-strong))' }}><Layers size={14}/>New Loadout</Button>
    {open ? <LoadoutFormDialog open={open} loadout={null} gatewayOptions={gateways.filter((gateway) => gateway.source !== 'in_process' && gateway.transport !== 'in_process').map((gateway) => ({ value: gateway.name, label: gateway.name, meta: gateway.config.url ?? gateway.config.command ?? gateway.transport }))} gatewayOptionsLoading={isLoading} gatewayOptionsError={error ? getErrorMessage(error, 'Server options could not be loaded') : null} serviceOptions={services.map((service) => ({ value: service.key, label: service.display_name, meta: service.description }))} onOpenChange={setOpen} onSave={async (_original, draft) => { await addLoadout(draft); toast.success(`Loadout ${draft.name} added.`) }}/> : null}
  </>
}
