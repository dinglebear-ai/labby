'use client'

import { Layers, Shield } from 'lucide-react'
import { cn } from '@/lib/utils'

export function ArtifactWorkspaceSwitch({ value, onChange, tabs = false }: {
  value: 'artifact' | 'bundle'
  onChange: (value: 'artifact' | 'bundle') => void
  tabs?: boolean
}) {
  return <div role="group" aria-label="Creation workspace" className={tabs ? 'flex h-[38px] gap-0.5 rounded-b-aurora-3 border-t border-aurora-border-default bg-aurora-control-surface px-5' : 'flex shrink-0 gap-[3px] rounded-full border border-aurora-border-default bg-aurora-control-surface p-[3px]'}>
    {(['artifact', 'bundle'] as const).map(mode => {
      const Icon = mode === 'artifact' ? Shield : Layers
      return <button key={mode} type="button" aria-pressed={value === mode} data-visible-label="1" title={mode === 'artifact' ? 'Create Artifact' : 'Create Bundle'} onClick={() => onChange(mode)} className={cn('inline-flex items-center whitespace-nowrap font-[650] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary', tabs ? 'h-[38px] gap-2 border-b-2 px-3.5 text-[12.5px]' : 'h-[26px] gap-1.5 rounded-full border px-[11px] text-[11.5px]', value === mode ? tabs ? 'border-aurora-accent-primary text-aurora-text-primary' : 'border-aurora-border-strong bg-aurora-selected-bg text-aurora-accent-strong' : 'border-transparent text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary')}><Icon className="size-[13px]" />{mode === 'artifact' ? 'Artifact' : 'Bundle'}</button>
    })}
  </div>
}
