'use client'

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { cn } from '@/lib/utils'

interface ActionConfirmationDialogProps {
  open: boolean
  title: string
  description: string
  confirmLabel: string
  cancelLabel?: string
  busy?: boolean
  onOpenChange: (open: boolean) => void
  onConfirm: () => void
}

export function ActionConfirmationDialog({
  open,
  title,
  description,
  confirmLabel,
  cancelLabel = 'Cancel',
  busy = false,
  onOpenChange,
  onConfirm,
}: ActionConfirmationDialogProps) {
  return (
    <AlertDialog open={open} onOpenChange={(nextOpen) => {
      if (busy && !nextOpen) return
      onOpenChange(nextOpen)
    }}>
      <AlertDialogContent className="border-aurora-border-strong bg-aurora-panel-strong text-aurora-text-primary">
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription className="text-aurora-text-muted">
            {description}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={busy}>{cancelLabel}</AlertDialogCancel>
          <AlertDialogAction
            disabled={busy}
            onClick={(event) => {
              event.preventDefault()
              onConfirm()
            }}
            className={cn(
              'bg-destructive text-white hover:bg-destructive/90 focus-visible:ring-destructive/30',
              busy && 'cursor-wait opacity-70',
            )}
          >
            {busy ? 'Working...' : confirmLabel}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
