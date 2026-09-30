import React from 'react'
import { Clipboard, Rocket, Save } from 'lucide-react'

import type { EditorDiagnostic, EditorLanguage } from '@/lib/editor/types'
import { Button } from './button'
import { TextSurfaceStatus } from './text-surface-status'
import { Tooltip, TooltipContent, TooltipTrigger } from './tooltip'

interface TextSurfaceToolbarProps {
  path: string
  language: EditorLanguage
  dirty?: boolean
  diagnostics?: EditorDiagnostic[]
  canEdit?: boolean
  onSave?: () => void
  onDeploy?: () => void
  onCopy?: () => void
}

export function TextSurfaceToolbar({ path, language, dirty = false, diagnostics = [], canEdit = false, onSave, onDeploy, onCopy }: TextSurfaceToolbarProps) {
  const action = (label: string, icon: React.ReactNode, onClick: () => void, primary = false) => (
    <Tooltip>
      <TooltipTrigger asChild><Button type="button" size="icon-sm" variant={primary ? 'default' : 'ghost'} aria-label={label} onClick={onClick}>{icon}</Button></TooltipTrigger>
      <TooltipContent sideOffset={7}>{label}</TooltipContent>
    </Tooltip>
  )
  return (
    <div className="flex items-center gap-3 border-b border-aurora-border-default bg-aurora-nav-bg px-4 py-3">
      <div className="min-w-0 flex-1">
        <div className="truncate font-mono text-xs text-aurora-text-primary">{path}</div>
        <div className="mt-1 text-[11px] uppercase tracking-[0.14em] text-aurora-text-muted">{language}</div>
      </div>
      <TextSurfaceStatus diagnostics={diagnostics} dirty={dirty} />
      {diagnostics[0] ? <span className="max-w-56 truncate text-xs text-aurora-text-muted">{diagnostics[0].message}</span> : null}
      {onCopy ? action('Copy source', <Clipboard />, onCopy) : null}
      {canEdit && onSave ? action('Save', <Save />, onSave) : null}
      {canEdit && onDeploy ? action('Deploy', <Rocket />, onDeploy, true) : null}
    </div>
  )
}
