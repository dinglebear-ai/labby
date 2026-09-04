'use client'

import { useEffect, useState } from 'react'
import { BookOpen, Code2, FileText, Loader2, MessageSquare, Wrench } from 'lucide-react'

import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Field, FieldContent, FieldDescription, FieldLabel, FieldTitle } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import type { GatewayLoadout, GatewayLoadoutInput } from '@/lib/types/gateway'
import { getErrorMessage } from '@/lib/utils'

export const LOADOUT_CAPABILITIES = [
  ['expose_tools', 'Tools', 'Direct MCP tool discovery and invocation.', Wrench],
  ['expose_resources', 'Resources', 'MCP resources and templates. Required by Agent Skills.', FileText],
  ['expose_prompts', 'Prompts', 'MCP prompt discovery and retrieval.', MessageSquare],
  ['expose_skills', 'Skills', 'Agent Skills discovery, retrieval, and manifest-bound files.', BookOpen],
  ['expose_code_mode', 'Code Mode', 'Curated gateway discovery and execution surface.', Code2],
] as const

type CapabilityKey = (typeof LOADOUT_CAPABILITIES)[number][0]
export type LoadoutOption = { value: string; label: string; meta?: string }

export function emptyLoadout(): GatewayLoadout {
  return {
    name: '', description: null, upstreams: [], services: [], expose_code_mode: false,
    expose_tools: true, expose_resources: true, expose_prompts: true, expose_skills: true,
  }
}

function uniq(values: string[]) { return [...new Set(values)].sort((a, b) => a.localeCompare(b)) }

export function loadoutSaveEnabled(
  saving: boolean,
  _gatewayOptionsLoading: boolean,
  _gatewayOptionsError: string | null,
  name: string,
  enabledCount: number,
  skillsNeedResources: boolean,
) {
  return !saving && name.trim().length > 0 && enabledCount > 0 && !skillsNeedResources
}

function SelectionGroup({ title, description, options, selected, onChange }: {
  title: string; description: string; options: LoadoutOption[]; selected: string[]; onChange: (v: string[]) => void
}) {
  const set = new Set(selected)
  return <div className="rounded-lg border bg-aurora-control-surface/10 p-3">
    <div className="mb-3 flex items-start justify-between gap-2">
      <div><p className="text-sm font-semibold text-aurora-text-primary">{title}</p><p className="mt-1 text-xs text-aurora-text-muted">{description}</p></div>
      {options.length > 0 && <Button type="button" size="sm" variant="outline" onClick={() => onChange(selected.length === options.length ? [] : options.map(x => x.value))}>{selected.length === options.length ? 'Clear' : 'Select all'}</Button>}
    </div>
    <div className="grid max-h-48 gap-2 overflow-y-auto pr-1 sm:grid-cols-2">
      {options.map(option => <label key={option.value} className="flex cursor-pointer items-start gap-3 rounded-md border bg-aurora-panel-medium/40 p-3 hover:bg-aurora-hover-bg">
        <Checkbox checked={set.has(option.value)} onCheckedChange={value => {
          const next = new Set(selected)
          if (value === true) {
            next.add(option.value)
          } else {
            next.delete(option.value)
          }
          onChange(uniq([...next]))
        }} />
        <span className="min-w-0"><span className="block text-sm font-medium text-aurora-text-primary">{option.label}</span>{option.meta && <span className="mt-0.5 block truncate text-xs text-aurora-text-muted">{option.meta}</span>}</span>
      </label>)}
      {options.length === 0 && <p className="text-sm text-aurora-text-muted">No options available.</p>}
    </div>
  </div>
}

export function LoadoutFormDialog({ open, loadout, gatewayOptions, gatewayOptionsLoading = false, gatewayOptionsError = null, serviceOptions, onOpenChange, onSave }: {
  open: boolean; loadout: GatewayLoadout | null; gatewayOptions: LoadoutOption[]; gatewayOptionsLoading?: boolean; gatewayOptionsError?: string | null; serviceOptions: LoadoutOption[];
  onOpenChange: (v: boolean) => void; onSave: (original: string | null, draft: GatewayLoadoutInput) => Promise<void>
}) {
  const [draft, setDraft] = useState<GatewayLoadout>(emptyLoadout())
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => { if (open) { setDraft(loadout ? { ...loadout, upstreams: [...loadout.upstreams], services: [...loadout.services] } : emptyLoadout()); setError(null) } }, [loadout, open])
  const skillsNeedResources = draft.expose_skills && !draft.expose_resources
  const enabledCount = LOADOUT_CAPABILITIES.filter(([key]) => draft[key]).length
  const canSave = loadoutSaveEnabled(saving, gatewayOptionsLoading, gatewayOptionsError, draft.name, enabledCount, skillsNeedResources)
  const cap = (key: CapabilityKey, value: boolean) => setDraft(current => ({ ...current, [key]: value }))

  return <Dialog open={open} onOpenChange={next => !saving && onOpenChange(next)}>
    <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-[760px]">
      <DialogHeader><DialogTitle>{loadout ? 'Edit Loadout' : 'New Loadout'}</DialogTitle><DialogDescription>Curate a portable bundle for agents and harnesses. You can export it through APM, publish it in Depot, or optionally host it behind a Labby route.</DialogDescription></DialogHeader>
      <div className="space-y-5 py-2">
        <div className="grid gap-4 sm:grid-cols-2">
          <Field><FieldLabel htmlFor="loadout-name">Name</FieldLabel><Input id="loadout-name" value={draft.name} onChange={e => setDraft(c => ({ ...c, name: e.target.value }))} placeholder="operations" /><FieldDescription>Portable identity used by APM, Depot, and optional route targets.</FieldDescription></Field>
          <Field><FieldLabel htmlFor="loadout-description">Description</FieldLabel><Textarea id="loadout-description" rows={3} value={draft.description ?? ''} onChange={e => setDraft(c => ({ ...c, description: e.target.value || null }))} placeholder="Operations-focused projection" /></Field>
        </div>
        <div className="grid gap-4 lg:grid-cols-2">
          <div className="space-y-2">
            <SelectionGroup title="MCP servers and tools" description="Bundle these servers; their allowed tools follow each server's exposure policy." options={gatewayOptions} selected={draft.upstreams} onChange={upstreams => setDraft(c => ({ ...c, upstreams }))} />
            {gatewayOptionsLoading && <p className="text-sm text-aurora-text-muted">Loading gateway options…</p>}
            {gatewayOptionsError && <p className="text-sm text-destructive">{gatewayOptionsError}</p>}
          </div>
          <SelectionGroup title="Lab plugins" description="Bundle built-in Labby service plugins with this portable Loadout." options={serviceOptions} selected={draft.services} onChange={services => setDraft(c => ({ ...c, services }))} />
        </div>
        <div className="grid gap-3 lg:grid-cols-2">{LOADOUT_CAPABILITIES.map(([key, label, description, Icon]) => <Field key={key} orientation="horizontal" className="rounded-lg border bg-aurora-control-surface/10 p-3"><Icon className="size-4 shrink-0 text-aurora-text-muted" /><FieldContent><FieldTitle>{label}</FieldTitle><FieldDescription>{description}</FieldDescription></FieldContent><Switch aria-label={label} checked={draft[key]} onCheckedChange={value => cap(key, value)} /></Field>)}</div>
        {skillsNeedResources && <div className="rounded-lg border border-destructive/40 bg-destructive/10 p-3 text-sm text-destructive">Agent Skills require Resources because skill files are read through MCP resources. Enable Resources or disable Skills.</div>}
        {enabledCount === 0 && <div className="rounded-lg border border-destructive/40 bg-destructive/10 p-3 text-sm text-destructive">Enable at least one capability category.</div>}
        {error && <div className="rounded-lg border border-destructive/40 bg-destructive/10 p-3 text-sm text-destructive">{error}</div>}
      </div>
      <DialogFooter><Button variant="outline" disabled={saving} onClick={() => onOpenChange(false)}>Cancel</Button><Button disabled={!canSave} onClick={async () => { setSaving(true); setError(null); try { await onSave(loadout?.name ?? null, { name: draft.name.trim(), description: draft.description?.trim() || null, upstreams: uniq(draft.upstreams), services: uniq(draft.services), expose_tools: draft.expose_tools, expose_resources: draft.expose_resources, expose_prompts: draft.expose_prompts, expose_skills: draft.expose_skills, expose_code_mode: draft.expose_code_mode }); onOpenChange(false) } catch (e) { setError(getErrorMessage(e, 'Failed to save Loadout')) } finally { setSaving(false) } }}>{saving && <Loader2 className="size-4 animate-spin" />}{loadout ? 'Save changes' : 'Create Loadout'}</Button></DialogFooter>
    </DialogContent>
  </Dialog>
}
