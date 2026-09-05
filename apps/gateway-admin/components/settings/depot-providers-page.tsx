'use client'

import { useEffect, useRef, useState } from 'react'
import { Loader2, Plus, RefreshCw, ShieldAlert } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { listProviders, type DepotProvider } from '@/lib/api/depot-client'
import { getBrowserSessionEpoch } from '@/lib/auth/session-store'
import { useBrowserSession } from '@/lib/auth/session'
import { SettingsCard, SettingsPageHeader } from './SettingsChrome'
import { DepotProviderDialog } from './depot-provider-dialog'

export function DepotProvidersPage() {
  const session = useBrowserSession()
  const [providers, setProviders] = useState<DepotProvider[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string>()
  const generation = useRef(0)
  const [editing, setEditing] = useState<DepotProvider | null | undefined>(undefined)

  function load() {
    const run = ++generation.current, epoch = getBrowserSessionEpoch()
    setLoading(true); setError(undefined)
    void listProviders().then(value => {
      if (run === generation.current && epoch === getBrowserSessionEpoch()) setProviders(value)
    }).catch(reason => {
      if (run === generation.current && epoch === getBrowserSessionEpoch()) setError(reason instanceof Error ? reason.message : 'Provider settings unavailable')
    }).finally(() => { if (run === generation.current && epoch === getBrowserSessionEpoch()) setLoading(false) })
  }

  useEffect(() => {
    if (session.status !== 'authenticated' || !session.isAdmin) { generation.current += 1; setProviders([]); return }
    load()
    return () => { generation.current += 1 }
    // load is intentionally tied to the current authority snapshot.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [session.status, session.status === 'authenticated' ? session.isAdmin : false])

  if (session.status === 'loading') return <p className="flex items-center gap-2 text-sm text-aurora-text-muted"><Loader2 className="size-4 animate-spin" />Checking permission…</p>
  if (session.status !== 'authenticated' || !session.isAdmin) return <div role="alert" className="flex gap-2 text-sm text-aurora-error"><ShieldAlert className="size-4" />Administrator permission is required for Depot provider settings.</div>
  return <div className="space-y-4">
    <SettingsPageHeader title="Depot providers" description="Manage the provider connections used by this Labby instance." />
    <SettingsCard title="Provider connections" action={<div className="flex gap-2"><Button size="sm" variant="outline" onClick={()=>setEditing(null)}><Plus className="size-4" />Add provider</Button><Button size="sm" variant="outline" disabled={loading} onClick={load}><RefreshCw className="size-4" />Refresh</Button></div>}>
      {error ? <p role="alert" className="p-4 text-sm text-aurora-error">{error}</p> : null}
      {loading && providers.length === 0 ? <p className="flex items-center gap-2 p-4 text-sm text-aurora-text-muted"><Loader2 className="size-4 animate-spin" />Loading providers…</p> : null}
      {providers.map(provider => <details key={provider.id} className="border-t border-aurora-border-subtle p-4 first:border-t-0">
        <summary className="cursor-pointer list-none"><span className="font-semibold text-aurora-text-primary">{provider.name}</span><span className="ml-2 text-xs text-aurora-text-muted">{provider.id}</span><span className="float-right flex gap-2"><Badge variant="outline">{provider.enabled ? 'enabled' : 'disabled'}</Badge><Badge variant="outline">{provider.health.state}</Badge></span></summary>
        <div className="mt-3 space-y-2 text-xs text-aurora-text-muted"><p className="break-all">{provider.endpoint}</p><p>{provider.credentialConfigured ? 'A server-held credential is configured.' : 'No credential is configured.'}</p>{provider.id === 'public' ? <p>The built-in Public Depot can be tested or disabled; its endpoint and credentials cannot be edited.</p> : <p>Credential values are never returned. Replacing, clearing, or moving credentials requires fresh authentication.</p>}<Button size="sm" variant="outline" onClick={()=>setEditing(provider)}>Manage</Button></div>
      </details>)}
    </SettingsCard>
    <p className="text-xs leading-5 text-aurora-text-muted">Enabling a shared bearer provider grants eligible users of this Labby instance read discovery through that credential. Removing it deletes only Labby&apos;s active copy; it does not revoke the upstream credential or erase recovery snapshots.</p>
    {editing !== undefined ? <DepotProviderDialog provider={editing ?? undefined} onClose={()=>setEditing(undefined)} onSaved={()=>{setEditing(undefined);load()}} /> : null}
  </div>
}
