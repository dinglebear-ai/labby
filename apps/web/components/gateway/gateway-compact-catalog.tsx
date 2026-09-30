'use client'

import { useMemo, useState } from 'react'
import { BookOpen, FileText, MessageSquare, Play, Search, Wrench } from 'lucide-react'
import { toast } from 'sonner'
import type { Gateway, GatewayConfig } from '@/lib/types/gateway'
import { cn, getErrorMessage } from '@/lib/utils'
import { readableTarget } from '@/lib/dashboard/readable-target'
import { GatewayToolInspector } from './gateway-tool-inspector'
import { DetailCard } from './gateway-detail-chrome'

type Kind = 'tools' | 'prompts' | 'resources' | 'skills'
type Entry = { name: string; description?: string; exposed: boolean }
export function GatewayCompactCatalog({ gateway, onSave, onAdvanced }: { gateway: Gateway; onSave: (config: Partial<GatewayConfig>) => Promise<unknown>; onAdvanced: () => void }) {
  const [descriptor, setDescriptor] = useState<Entry | null>(null)
  const [inspectTarget, setInspectTarget] = useState<string | null>(null)
  const [kind, setKind] = useState<Kind>('tools')
  const [search, setSearch] = useState('')
  const [filter, setFilter] = useState<'all' | 'exposed' | 'hidden'>('all')
  const [saving, setSaving] = useState(false)
  const inventory: Record<Kind, Entry[]> = useMemo(() => ({ tools: gateway.discovery.tools, prompts: gateway.discovery.prompts.map((row) => ({ ...row, exposed: Boolean(row.exposed) })), resources: gateway.discovery.resources.map((row) => ({ ...row, exposed: Boolean(row.exposed) })), skills: [] }), [gateway.discovery])
  const exposureEnabled = kind === 'resources' ? Boolean(gateway.config.proxy_resources) : kind === 'prompts' ? Boolean(gateway.config.proxy_prompts) : kind === 'skills' ? Boolean(gateway.config.proxy_skills) : true
  const items = inventory[kind]
  const visible = items.filter((item) => (filter === 'all' || item.exposed === (filter === 'exposed')) && `${item.name} ${item.description ?? ''}`.toLowerCase().includes(search.toLowerCase()))
  const save = async (names: string[]) => {
    setSaving(true)
    try { await onSave({ [`expose_${kind}`]: names }); toast.success('Catalog exposure updated') }
    catch (error) { toast.error(getErrorMessage(error, 'Could not update catalog exposure')) }
    finally { setSaving(false) }
  }
  return <><GatewayToolInspector target={inspectTarget} descriptor={descriptor} onClose={() => { setInspectTarget(null); setDescriptor(null) }}/><DetailCard padding="0" className="overflow-hidden">
    <div className="flex flex-wrap items-center gap-3 px-3.5 py-3" style={{ background: 'var(--gw0-0_38)' }}>
      <label className="relative min-w-44 flex-1"><Search size={13} className="absolute left-2.5 top-2.5 text-aurora-text-muted"/><input aria-label="Search server catalog" placeholder="Search tools, resources, prompts…" value={search} onChange={(event) => setSearch(event.target.value)} className="h-8 w-full rounded-lg border border-aurora-border-default bg-aurora-page-bg pl-8 pr-3 text-xs"/></label>
      <div className="flex gap-1">{(['all', 'exposed', 'hidden'] as const).map((value) => <button key={value} type="button" aria-pressed={filter === value} onClick={() => setFilter(value)} className={cn('rounded-lg border px-2.5 py-1 text-[11px] capitalize', filter === value ? 'border-aurora-accent-primary/40 bg-aurora-selected-bg text-aurora-accent-strong' : 'border-transparent text-aurora-text-muted')}>{value}</button>)}</div>
    </div>
    <div className="flex gap-1 border-y border-aurora-border-default px-3.5">{([['tools', Wrench], ['prompts', MessageSquare], ['resources', FileText], ['skills', BookOpen]] as const).map(([value, Icon]) => <button key={value} type="button" aria-pressed={kind === value} onClick={() => setKind(value)} className={cn('inline-flex items-center gap-1.5 border-b-2 px-2.5 py-2 text-[11px] capitalize', kind === value ? 'border-aurora-accent-primary text-aurora-accent-strong' : 'border-transparent text-aurora-text-muted')}><Icon size={12}/>{value}<span className="text-[10px] tabular-nums">{value === 'skills' ? gateway.status.discovered_skill_count ?? 0 : inventory[value].length}</span></button>)}</div>
    {visible.map((item) => <div key={item.name} className="flex min-h-[39px] items-center gap-3 border-b border-aurora-border-subtle px-3.5 py-1.5">
      {kind === 'tools' ? <Wrench size={12} className="shrink-0 text-aurora-text-muted"/> : kind === 'prompts' ? <MessageSquare size={12} className="shrink-0 text-aurora-text-muted"/> : <FileText size={12} className="shrink-0 text-aurora-text-muted"/>}<button type="button" title={item.name} onClick={() => { setDescriptor(kind === 'tools' ? null : item); setInspectTarget(`${gateway.name}::${item.name}`) }} aria-label={`Inspect ${item.name}`} className="shrink-0 font-mono text-[11px] font-semibold hover:text-aurora-accent-strong">{readableTarget(item.name)}</button><span className="min-w-0 flex-1 truncate text-[11px] text-aurora-text-muted" title={item.description}>{item.description ?? '—'}</span>
      <span className="text-[10px] text-aurora-text-muted" title="The discovered catalog does not report schema token estimates">— tok</span>
      <button type="button" disabled title="Browser execution is not supported; inspect the live definition and execute through an MCP client" aria-label={`Run ${item.name}: browser execution unavailable`} className="text-aurora-text-muted opacity-40"><Play size={12}/></button>
      <button type="button" disabled={saving || !exposureEnabled} title={exposureEnabled ? undefined : `Enable ${kind} proxying in server settings first`} onClick={() => void save(item.exposed ? items.filter((row) => row.exposed && row.name !== item.name).map((row) => row.name) : [...items.filter((row) => row.exposed).map((row) => row.name), item.name])} aria-pressed={item.exposed} aria-label={`${item.exposed ? 'Hide' : 'Expose'} ${item.name}`} className={cn('rounded-md px-1.5 py-0.5 text-[9px] font-semibold uppercase', item.exposed ? 'bg-aurora-success/10 text-aurora-success' : 'bg-aurora-control-surface text-aurora-text-muted')}>{item.exposed ? 'Exposed' : 'Hidden'}</button>
    </div>)}
    {!visible.length ? <p className="px-4 py-10 text-center text-xs text-aurora-text-muted">{kind === 'skills' && (gateway.status.discovered_skill_count ?? 0) > 0 ? 'The server reports a skill count but does not return individual skill entries here. Use the exposure editor to manage patterns.' : items.length ? 'No entries match these filters.' : 'Nothing of this type discovered.'}</p> : null}
    <div className="flex items-center justify-between gap-3 px-3.5 py-2 text-[10.5px] text-aurora-text-muted" style={{ background: 'var(--gw0-0_38)' }}><span>{visible.length} shown · {items.filter((item) => item.exposed).length} exposed</span><div className="flex gap-3"><button type="button" onClick={onAdvanced}>Exposure editor</button><button type="button" className="text-aurora-accent-strong disabled:opacity-40" disabled={saving || !items.length || !exposureEnabled} onClick={() => void save(items.map((item) => item.name))}>Expose all</button></div></div>
  </DetailCard></>
}
