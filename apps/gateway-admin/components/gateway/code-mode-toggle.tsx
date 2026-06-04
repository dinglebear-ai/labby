'use client'

import { useRef, useState } from 'react'
import { Code2 } from 'lucide-react'
import { mutate } from 'swr'
import { toast } from 'sonner'

import { AURORA_STRONG_PANEL } from '@/components/aurora/tokens'
import { Badge } from '@/components/ui/badge'
import { Switch } from '@/components/ui/switch'
import {
  useGatewayCodeModeConfig,
  useGatewayMutations,
  CODE_MODE_CONFIG_KEY,
} from '@/lib/hooks/use-gateways'
import { cn, getErrorMessage } from '@/lib/utils'

export function CodeModeTogglePanel() {
  const { data: codeModeConfig, isLoading, error } = useGatewayCodeModeConfig()
  const { setCodeModeConfig } = useGatewayMutations()
  const isSavingRef = useRef(false)
  const [isSaving, setIsSaving] = useState(false)
  // Code Mode can be toggled whenever it has data and is not already saving.
  const canToggle = Boolean(codeModeConfig) && !isLoading && !isSaving

  async function handleToggle(enabled: boolean) {
    if (!codeModeConfig || isSavingRef.current) return
    isSavingRef.current = true
    setIsSaving(true)
    try {
      await setCodeModeConfig({ enabled })
      toast.success(enabled ? 'Code Mode enabled.' : 'Code Mode disabled.')
    } catch (requestError) {
      toast.error(getErrorMessage(requestError, 'Failed to update Code Mode'))
      // Re-fetch config so UI reflects actual server state after a failure.
      await mutate(CODE_MODE_CONFIG_KEY)
    } finally {
      isSavingRef.current = false
      setIsSaving(false)
    }
  }

  return (
    <section
      className={cn(AURORA_STRONG_PANEL, 'flex flex-col divide-y divide-aurora-border-subtle px-5 py-1')}
    >
      <div className="flex flex-col gap-3 py-3 lg:flex-row lg:items-center lg:justify-between">
        <div className="flex items-start gap-3">
          <Code2 className="mt-0.5 size-5 text-aurora-accent-primary" />
          <div className="min-w-0">
            <p className="text-sm font-semibold text-aurora-text-primary">Code Mode</p>
            <p className="mt-1 text-sm text-aurora-text-muted">
              Expose server-wide{' '}
              <code className="rounded bg-aurora-panel-strong px-1.5 py-0.5 text-aurora-text-primary">
                search
              </code>{' '}
              and{' '}
              <code className="rounded bg-aurora-panel-strong px-1.5 py-0.5 text-aurora-text-primary">
                execute
              </code>{' '}
              instead of listing every upstream tool directly.
            </p>
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-3">
          {error ? (
            <p className="text-xs text-aurora-error">Code Mode settings unavailable.</p>
          ) : (
            <div className="flex flex-wrap gap-2">
              <Badge variant="secondary" status={codeModeConfig?.enabled ? 'success' : 'default'}>
                {codeModeConfig?.enabled ? 'Enabled' : 'Disabled'}
              </Badge>
              {isSaving ? <Badge variant="outline">Saving</Badge> : null}
            </div>
          )}
          <Switch
            aria-label="Code Mode"
            checked={codeModeConfig?.enabled ?? false}
            disabled={!canToggle}
            onCheckedChange={handleToggle}
          />
        </div>
      </div>
      <div className="flex flex-col gap-3 py-3 lg:flex-row lg:items-center lg:justify-between">
        <div className="flex items-start gap-3">
          <Code2 className="mt-0.5 size-5 text-aurora-accent-secondary" />
          <div className="min-w-0">
            <p className="text-sm font-semibold text-aurora-text-primary">Code execution limits</p>
            <p className="mt-1 text-sm text-aurora-text-muted">
              Tuning knobs applied when the{' '}
              <code className="rounded bg-aurora-panel-strong px-1.5 py-0.5 text-aurora-text-primary">
                execute
              </code>{' '}
              surface runs code — the constrained JS sandbox honors these bounds per execution.
            </p>
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-3">
          {error ? (
            <p className="text-xs text-aurora-error">Code execution limits unavailable.</p>
          ) : (
            <div className="flex flex-wrap gap-2">
              <Badge variant="outline">{codeModeConfig?.max_tool_calls ?? '-'} calls</Badge>
              <Badge variant="outline">{codeModeConfig?.timeout_ms ?? '-'}ms</Badge>
              <Badge variant="outline">
                {codeModeConfig?.max_response_tokens ?? '-'} tokens
              </Badge>
            </div>
          )}
        </div>
      </div>
    </section>
  )
}
