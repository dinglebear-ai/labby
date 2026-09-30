import React from 'react'

import type { EditorDiagnostic } from '@/lib/editor/types'
import { cn } from '@/lib/utils'

export function TextSurfaceStatus({ diagnostics = [], dirty = false }: { diagnostics?: EditorDiagnostic[]; dirty?: boolean }) {
  const top = diagnostics[0]
  const tone = top?.severity === 'error'
    ? 'text-aurora-error border-[color-mix(in_srgb,var(--aurora-error)_30%,transparent)] bg-[color-mix(in_srgb,var(--aurora-error)_10%,transparent)]'
    : top?.severity === 'warning'
      ? 'text-aurora-warn border-[color-mix(in_srgb,var(--aurora-warn)_30%,transparent)] bg-[color-mix(in_srgb,var(--aurora-warn)_10%,transparent)]'
      : 'text-aurora-success border-[color-mix(in_srgb,var(--aurora-success)_30%,transparent)] bg-[color-mix(in_srgb,var(--aurora-success)_10%,transparent)]'

  if (!top && !dirty) {
    return (
      <span className="inline-flex items-center rounded-full border border-aurora-border-default bg-aurora-control-surface px-2 py-1 text-[11px] font-semibold uppercase tracking-[0.14em] text-aurora-text-muted">
        Clean
      </span>
    )
  }

  return (
    <span className={cn('inline-flex items-center rounded-full border px-2 py-1 text-[11px] font-semibold uppercase tracking-[0.14em]', tone)}>
      {dirty ? 'Dirty' : top?.severity ?? 'Info'}
    </span>
  )
}
