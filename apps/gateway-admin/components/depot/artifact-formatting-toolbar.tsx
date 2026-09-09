'use client'

import { Bold, Code, Code2, Heading2, Italic, List, ListOrdered, ChevronDown } from 'lucide-react'
import type { ArtifactFormat } from '@/lib/editor/artifact-formatting'
import type { ReactNode } from 'react'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'

const ACTIONS = [
  ['heading', 'Heading', Heading2], ['bold', 'Bold', Bold], ['italic', 'Italic', Italic],
  ['inlineCode', 'Inline code', Code2], ['bullet', 'Bulleted list', List], ['numbered', 'Numbered list', ListOrdered], ['code', 'Code block', Code],
] as const

export function ArtifactFormattingToolbar({ onFormat, onSection, indicator }: { onFormat: (format: ArtifactFormat) => void; onSection: (section: string) => void; indicator?: ReactNode }) {
  return <div role="group" aria-label="Format artifact content" className="flex flex-wrap items-center gap-1 border-b border-aurora-border-subtle bg-aurora-page-bg/30 py-1.5 pl-4 pr-3">
    {indicator}
    {ACTIONS.map(([format, label, Icon]) => <button key={format} type="button" aria-label={label} title={label} onMouseDown={event => event.preventDefault()} onClick={() => onFormat(format)} className="grid size-[26px] place-items-center rounded-[7px] text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary"><Icon className="size-[13px]" /></button>)}
    <span aria-hidden="true" className="mx-1.5 h-4 w-px bg-aurora-border-default" />
    <DropdownMenu>
      <DropdownMenuTrigger aria-label="Insert a section" data-visible-label="1" style={{ lineHeight: 'normal' }} className="inline-flex h-6 items-center gap-[5px] rounded-full border border-aurora-border-strong px-[9px] text-[10.5px] font-[650] text-aurora-text-muted hover:text-aurora-accent-strong focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary"><span className="font-mono opacity-60" style={{ lineHeight: 'normal' }}>/</span>Section<ChevronDown style={{ width: 10, height: 10 }} /></DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="min-w-[170px] rounded-xl p-1">
        {['When to use', 'Steps', 'Examples', 'Constraints'].map(section => <DropdownMenuItem key={section} onSelect={() => onSection(section)} className="text-[11.5px]">{section}</DropdownMenuItem>)}
      </DropdownMenuContent>
    </DropdownMenu>
  </div>
}
