'use client'

import { useRef, useState } from 'react'
import { Code2 } from 'lucide-react'
import { mutate } from 'swr'
import { toast } from 'sonner'

import { Switch } from '@/components/ui/switch'
import {
  useGatewayCodeModeConfig,
  useGatewayMutations,
  CODE_MODE_CONFIG_KEY,
} from '@/lib/hooks/use-gateways'
import { cn, getErrorMessage } from '@/lib/utils'
import { gatewayActionTone } from './gateway-theme'

export function CodeModeHeaderToggle() {
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
    <div
      className={cn(
        gatewayActionTone(),
        'hidden h-10 items-center gap-2 rounded-aurora-1 px-2.5 lg:inline-flex',
        error && 'border-aurora-error/45 text-aurora-error',
        codeModeConfig?.enabled && 'border-aurora-accent-primary/45 text-aurora-accent-strong',
      )}
      title={error ? 'Code Mode settings unavailable' : `Code Mode ${codeModeConfig?.enabled ? 'enabled' : 'disabled'}`}
      aria-label={error ? 'Code Mode settings unavailable' : `Code Mode ${codeModeConfig?.enabled ? 'enabled' : 'disabled'}`}
      aria-busy={isSaving}
    >
      <Code2 className={cn('size-4', isSaving && 'animate-pulse')} />
      <Switch
        aria-label="Toggle Code Mode"
        checked={codeModeConfig?.enabled ?? false}
        disabled={!canToggle}
        onCheckedChange={handleToggle}
      />
    </div>
  )
}
