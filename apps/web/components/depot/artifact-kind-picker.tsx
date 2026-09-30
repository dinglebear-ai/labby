'use client'

import { Check, ChevronDown } from 'lucide-react'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { ARTIFACT_KINDS, type ArtifactKind } from '@/lib/editor/artifact-standards'
import { artifactTypeDefinition } from './artifact-type'

const DESCRIPTIONS: Record<ArtifactKind, string> = {
  Skill: 'Progressive-disclosure instructions.',
  Agent: 'A persona with tools and a loop.',
  Command: 'A slash command with arguments.',
  Hook: 'Lifecycle guard around a session.',
  Prompt: 'A reusable, parameterized prompt.',
  MCP: 'A server config and its scoped tools.',
  Plugin: 'An editable plugin JSON document.',
  Loadout: 'An editable loadout JSON document.',
}

export function ArtifactKindPicker({ value, onChange }: { value: ArtifactKind; onChange: (kind: ArtifactKind) => void }) {
  const selected = artifactTypeDefinition(value)
  const Icon = selected.icon
  return <DropdownMenu>
    <DropdownMenuTrigger aria-label={`Change artifact kind: ${value}`} data-visible-label="1" style={selected.iconStyle} className="inline-flex h-7 shrink-0 items-center gap-[7px] rounded-lg border pl-[7px] pr-2.5 text-[10.5px] font-bold uppercase tracking-[.1em] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary">
      <Icon aria-hidden="true" className="size-[13px]" />{value}<ChevronDown aria-hidden="true" className="size-2.5" />
    </DropdownMenuTrigger>
    <DropdownMenuContent align="start" className="w-[272px] max-w-[calc(100vw-32px)] rounded-xl border-aurora-border-default bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] p-[5px] shadow-aurora-strong">
      {ARTIFACT_KINDS.map(kind => {
        const definition = artifactTypeDefinition(kind)
        const KindIcon = definition.icon
        return <DropdownMenuItem key={kind} onSelect={() => onChange(kind)} className="items-start gap-[9px] rounded-lg px-[9px] py-[7px]">
          <span style={definition.iconStyle} className="grid size-[22px] shrink-0 place-items-center rounded-[7px] border"><KindIcon aria-hidden="true" className="size-[13px]" /></span>
          <span className="flex min-w-0 flex-1 flex-col gap-px"><span className="text-[12.5px] font-[650] text-aurora-text-primary">{kind}</span><span className="text-[10px] leading-[1.4] text-aurora-text-muted">{DESCRIPTIONS[kind]}</span></span>
          {value === kind ? <Check aria-hidden="true" className="mt-[3px] size-3 shrink-0 text-aurora-accent-strong" /> : null}
        </DropdownMenuItem>
      })}
    </DropdownMenuContent>
  </DropdownMenu>
}
