'use client'

import { useEffect, useId, useRef, useState } from 'react'
import { ReauthDialog } from '@/components/auth/reauth-dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { probeProvider, providerOperation, removeProvider, upsertProvider, type CredentialOperation, type DepotProvider } from '@/lib/api/depot-client'
import { useBrowserSession } from '@/lib/auth/session'

type Props = { provider?: DepotProvider; baseVersion: string; onSaved(): void; onClose(): void }
type ReauthAction = 'save' | 'remove'

export function initialProviderAuthMode(provider?: DepotProvider): 'anonymous'|'bearer' {
  return provider?.authMode ?? 'anonymous'
}

export function providerRequiresFreshProof(provider: DepotProvider | undefined, endpoint: string, authMode: 'anonymous'|'bearer', credential: 'retain'|'replace'|'clear') {
  if (provider?.id === 'public') return false
  if (!provider) return authMode === 'bearer'
  return credential !== 'retain' || provider.endpoint !== endpoint || provider.authMode !== authMode
}

export function providerInputError(id: string, name: string, endpoint: string, builtin = false): string | undefined {
  if (!builtin && (!/^[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?$/.test(id) || ['all', 'legacy', 'public'].includes(id))) {
    return 'Choose an ID of 1–64 lowercase letters, numbers, or hyphens. Start and end with a letter or number; all, legacy, and public are reserved.'
  }
  if (Array.from(name).length < 1 || Array.from(name).length > 128 || !name.trim()) return 'Enter a display name of 1–128 characters.'
  try {
    const url = new URL(endpoint)
    if (new TextEncoder().encode(endpoint).length > 2048 || endpoint.trim() !== endpoint || url.protocol !== 'https:' || !url.hostname || url.username || url.password || url.search || url.hash) throw new Error('invalid')
  } catch {
    return 'Enter a full HTTPS catalog URL without credentials, query, or fragment. Local catalog connections are managed separately on the server.'
  }
  return undefined
}

export function DepotProviderDialog({ provider, baseVersion, onSaved, onClose }: Props) {
  const session = useBrowserSession(), title = useId(), secret = useRef<HTMLInputElement>(null)
  const generation = useRef(0), operation = useRef(crypto.randomUUID())
  const [id,setId]=useState(provider?.id??''), [name,setName]=useState(provider?.name??''), [endpoint,setEndpoint]=useState(provider?.endpoint??'')
  const [enabled,setEnabled]=useState(provider?.enabled??false), [authMode,setAuthMode]=useState<'anonymous'|'bearer'>(()=>initialProviderAuthMode(provider))
  const [credential,setCredential]=useState<'retain'|'replace'|'clear'>('retain'), [error,setError]=useState<string>(), [busy,setBusy]=useState(false), [probe,setProbe]=useState<string>()
  const [proof,setProof]=useState<string>(), [reauthAction,setReauthAction]=useState<ReauthAction>('save'), [reauthOpen,setReauthOpen]=useState(false)
  const builtin=provider?.id==='public', version=provider?.configVersion??baseVersion
  const needsProof=providerRequiresFreshProof(provider,endpoint,authMode,credential)

  useEffect(()=>()=>{ if(secret.current)secret.current.value=''; generation.current+=1 },[])
  const credentialValue=():CredentialOperation=>credential==='replace'?{action:'replace',value:secret.current?.value??''}:credential==='clear'?{action:'clear'}:{action:'retain'}
  const reconcile=async(operationId:string,reason:unknown)=>{const outcome=await providerOperation(operationId).catch(()=>null);if(!outcome?.committed)throw reason}

  const run=async(kind:'probe'|'save')=>{
    const invalid = providerInputError(id, name, endpoint, builtin)
    if (invalid) { setError(invalid); return }
    if (authMode === 'bearer' && credential === 'replace' && !secret.current?.value.trim()) { setError('Enter the new bearer token.'); return }
    const runGeneration=++generation.current;setBusy(true);setError(undefined);setProbe(undefined)
    try {
      const csrf=session.status==='authenticated'?session.csrfToken:''
      if(kind==='probe'){const result=await probeProvider({id,name,endpoint,enabled,authMode,credential:credentialValue()},csrf);if(runGeneration===generation.current)setProbe(result.state);return}
      if(needsProof&&!proof)throw new Error('Fresh authentication is required before saving provider credentials.')
      try{await upsertProvider({id,name,endpoint,enabled,authMode,credential:credentialValue(),expectedVersion:version,operationId:operation.current,proof},csrf)}catch(reason){await reconcile(operation.current,reason)}
      if(runGeneration===generation.current)onSaved()
    } catch(reason){if(runGeneration===generation.current)setError(reason instanceof Error?reason.message:'Provider operation failed')}
    finally{if(secret.current)secret.current.value='';if(runGeneration===generation.current)setBusy(false)}
  }

  const remove=async(freshProof:string)=>{
    if(!provider||builtin)return
    const runGeneration=++generation.current;setBusy(true);setError(undefined)
    try {
      const csrf=session.status==='authenticated'?session.csrfToken:''
      try{await removeProvider(provider.id,version,operation.current,freshProof,csrf)}catch(reason){await reconcile(operation.current,reason)}
      if(runGeneration===generation.current)onSaved()
    } catch(reason){if(runGeneration===generation.current)setError(reason instanceof Error?reason.message:'Provider removal failed')}
    finally{if(runGeneration===generation.current)setBusy(false)}
  }

  const openReauth=(action:ReauthAction)=>{operation.current=crypto.randomUUID();setProof(undefined);setReauthAction(action);setReauthOpen(true)}
  const purpose=reauthAction==='remove'
    ? {action:'providers.remove',resource:id,version,operation:operation.current,scope:'lab:admin',payload:{providerId:id}}
    : {action:'providers.upsert',resource:id,version,operation:operation.current,scope:'lab:admin',payload:{id,name,endpoint,enabled,authMode,credential:credential==='replace'?'replace':credential}}

  return <div role="dialog" aria-modal="true" aria-labelledby={title} className="fixed inset-0 z-50 grid place-items-center bg-black/45 p-4">
    <div className="max-h-[92vh] w-full max-w-xl overflow-y-auto aurora-scrollbar rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-strong p-5 shadow-xl">
      <h2 id={title} className="text-lg font-semibold">{provider?'Edit provider':'Add provider'}</h2>
      <p className="mt-1 text-xs text-aurora-text-muted">Connect a catalog that Discover can search from the Labby server. Credentials stay on that server and are never returned to this form.</p>
      {error?<p role="alert" tabIndex={-1} className="mt-3 text-sm text-aurora-error">{error}</p>:null}
      <div className="mt-4 grid gap-4">
        {!provider?<label className="text-sm">Provider ID<Input required maxLength={64} pattern="[a-z0-9]([a-z0-9-]*[a-z0-9])?" value={id} onChange={event=>setId(event.target.value)}/><span className="text-xs text-aurora-text-muted">Stable catalog ID: lowercase letters, numbers, and hyphens. It cannot change after creation.</span></label>:null}
        <label className="text-sm">Display name<Input required maxLength={128} value={name} disabled={builtin} onChange={event=>{setName(event.target.value);setProof(undefined);setProbe(undefined)}}/></label>
        <label className="text-sm">Catalog URL<Input required maxLength={2048} type="url" value={endpoint} disabled={builtin} onChange={event=>{setEndpoint(event.target.value);setProof(undefined);setProbe(undefined)}}/><span className="text-xs text-aurora-text-muted">Full HTTPS address reachable from the Labby server, for example https://catalog.example.com.</span></label>
        <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={enabled} onChange={event=>setEnabled(event.target.checked)}/>Show this catalog in Discover</label>
        {!builtin?<><label className="text-sm">Authentication<select className="ml-2 rounded border p-2" value={authMode} onChange={event=>{const mode = event.target.value as 'anonymous'|'bearer';setAuthMode(mode);setCredential(mode === 'bearer' && !provider?.credentialConfigured ? 'replace' : 'retain');setProof(undefined);setProbe(undefined)}}><option value="anonymous">No credential (public access)</option><option value="bearer">Bearer token</option></select></label>{authMode==='bearer'?<fieldset className="space-y-2"><legend className="text-sm">Credential</legend>{(['retain','replace','clear'] as const).map(value=><label key={value} className="mr-3 text-sm"><input type="radio" name="credential" checked={credential===value} onChange={()=>{setCredential(value);setProof(undefined);setProbe(undefined)}}/> {{retain:'Keep saved token',replace:'Replace token',clear:'Remove saved token'}[value]}</label>)}{credential==='replace'?<Input ref={secret} type="password" autoComplete="new-password" aria-label="New bearer token"/>:null}</fieldset>:null}</>:null}
        <p className="text-xs text-aurora-text-muted">The token lets eligible Labby users search this catalog through this server. Catalog access does not authorize installing or running listed servers.</p>
        {provider&&!builtin?<p className="rounded-aurora-1 border border-aurora-warn/35 p-3 text-xs text-aurora-text-muted">Removing this provider stops its discovery results and invalidates its Labby links. Labby deletes only its stored credential; it does not revoke the token at the provider.</p>:null}
        {probe?<p role="status" className="text-sm">Diagnostic result: {probe}</p>:null}
        <div className="flex flex-wrap justify-end gap-2">
          {provider&&!builtin?<Button variant="destructive" disabled={busy} onClick={()=>openReauth('remove')}>Remove</Button>:null}
          <Button variant="ghost" onClick={()=>{if(secret.current)secret.current.value='';onClose()}}>Cancel</Button>
          <Button variant="outline" disabled={busy} onClick={()=>void run('probe')}>Test catalog access</Button>
          {needsProof?<Button variant="outline" onClick={()=>openReauth('save')}>{proof?'Identity confirmed':'Confirm identity'}</Button>:null}
          <Button disabled={busy||(needsProof&&!proof)} onClick={()=>void run('save')}>Save</Button>
        </div>
      </div>
    </div>
    <ReauthDialog open={reauthOpen} purpose={purpose} onOpenChange={setReauthOpen} onProof={freshProof=>{if(reauthAction==='remove')void remove(freshProof);else setProof(freshProof)}}/>
  </div>
}
