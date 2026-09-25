'use client'

import * as React from 'react'
import { AlertCircle, FileText, Loader2, Pencil } from 'lucide-react'

import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import { cn } from '@/lib/utils'

interface ServerInstructionsEditorProps {
  value?: string | null
  fallback?: string | null
  fallbackLabel?: string
  title?: string
  description?: string
  className?: string
  disabled?: boolean
  onSave: (instructions: string | null) => Promise<void>
}

export function ServerInstructionsEditor({
  value,
  fallback,
  fallbackLabel = 'Server default',
  title = 'Server instructions',
  description = 'These instructions are sent to MCP clients when they initialize this server.',
  className,
  disabled = false,
  onSave,
}: ServerInstructionsEditorProps) {
  const [open, setOpen] = React.useState(false)
  const [draft, setDraft] = React.useState('')
  const [saving, setSaving] = React.useState(false)
  const [error, setError] = React.useState<string | null>(null)

  const override = value?.trim() || null
  const fallbackValue = fallback?.trim() || null
  const effective = override ?? fallbackValue
  const source = override ? 'Custom override' : fallbackValue ? fallbackLabel : 'Not configured'

  React.useEffect(() => {
    if (open) {
      setDraft(effective ?? '')
      setError(null)
    }
  }, [open, effective])

  async function save(next: string | null) {
    setSaving(true)
    setError(null)
    try {
      await onSave(next)
      setOpen(false)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Failed to update server instructions')
    } finally {
      setSaving(false)
    }
  }

  return (
    <>
      <button
        type="button"
        disabled={disabled}
        onClick={() => setOpen(true)}
        className={cn(
          'group w-full rounded-aurora-2 border border-aurora-border-default bg-aurora-control-surface/45 px-3.5 py-3 text-left transition-colors',
          'hover:border-aurora-border-strong hover:bg-aurora-hover-bg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary/40',
          'disabled:cursor-not-allowed disabled:opacity-60',
          className,
        )}
      >
        <span className="flex items-center gap-2">
          <FileText className="size-3.5 shrink-0 text-aurora-text-muted" />
          <span className="text-[10px] font-bold uppercase tracking-[0.12em] text-aurora-text-muted">
            Server instructions
          </span>
          <span className="rounded-full border border-aurora-border-default px-2 py-0.5 text-[10px] font-medium text-aurora-text-muted">
            {source}
          </span>
          <Pencil className="ml-auto size-3.5 text-aurora-text-muted transition-colors group-hover:text-aurora-text-primary" />
        </span>
        <span className="mt-1.5 line-clamp-2 block text-xs leading-5 text-aurora-text-secondary">
          {effective ?? 'No server instructions are configured. Click to add them.'}
        </span>
      </button>

      <Dialog open={open} onOpenChange={(next) => { if (!saving) setOpen(next) }}>
        <DialogContent className="sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle>{title}</DialogTitle>
            <DialogDescription>{description}</DialogDescription>
          </DialogHeader>

          <div className="space-y-3">
            <div className="flex items-center justify-between gap-3">
              <Label htmlFor="server-instructions">Instructions</Label>
              <span className="text-xs text-aurora-text-muted">{source}</span>
            </div>
            <Textarea
              id="server-instructions"
              autoFocus
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
              disabled={saving}
              rows={12}
              className="min-h-64 resize-y font-mono text-xs leading-5"
              placeholder="Tell MCP clients how to use this server…"
            />
            <p className="text-xs leading-5 text-aurora-text-muted">
              Changes apply to new MCP initialize/discover responses immediately. No server restart is required.
            </p>
            {error ? (
              <div role="alert" className="flex items-start gap-2 rounded-aurora-1 border border-aurora-error/30 bg-aurora-error/5 p-3 text-sm text-aurora-error">
                <AlertCircle className="mt-0.5 size-4 shrink-0" />
                <span>{error}</span>
              </div>
            ) : null}
          </div>

          <DialogFooter className="gap-2 sm:gap-0">
            {fallbackValue && override ? (
              <Button variant="ghost" disabled={saving} onClick={() => void save(null)} className="sm:mr-auto">
                Use {fallbackLabel.toLowerCase()}
              </Button>
            ) : null}
            <Button variant="outline" disabled={saving} onClick={() => setOpen(false)}>Cancel</Button>
            <Button
              disabled={saving || draft.trim() === (effective ?? '')}
              onClick={() => void save(draft.trim() || null)}
            >
              {saving ? <Loader2 className="size-4 animate-spin" /> : null}
              Save instructions
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}
