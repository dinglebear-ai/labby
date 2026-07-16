'use client'

import { useState } from 'react'

import { ActionConfirmationDialog } from '@/components/action-confirmation-dialog'
import { Switch } from '@/components/ui/switch'

interface GatewayEnabledSettingProps {
  enabled: boolean
  onEnable: () => void | Promise<void>
  onDisable: () => void | Promise<void>
}

export function GatewayEnabledSetting({
  enabled,
  onEnable,
  onDisable,
}: GatewayEnabledSettingProps) {
  const [disableConfirmationOpen, setDisableConfirmationOpen] = useState(false)

  const handleCheckedChange = (checked: boolean) => {
    if (checked) {
      void onEnable()
      return
    }
    setDisableConfirmationOpen(true)
  }

  const confirmDisable = () => {
    setDisableConfirmationOpen(false)
    if (enabled) void onDisable()
  }

  return (
    <>
      <div className="flex items-start justify-between gap-4 rounded-lg border bg-aurora-control-surface/10 p-4">
        <div className="min-w-0">
          <p className="text-sm font-semibold text-aurora-text-primary">Server enabled</p>
          <p className="mt-1 text-sm text-aurora-text-muted">
            Controls whether this server participates in the active catalog and serves tools, resources, and prompts.
          </p>
        </div>
        <Switch
          aria-label="Server enabled"
          checked={enabled}
          onCheckedChange={handleCheckedChange}
        />
      </div>
      <ActionConfirmationDialog
        open={disableConfirmationOpen}
        title="Disable server?"
        description="Connected clients should no longer have access to this server. Existing sessions may fail until the gateway is enabled again."
        confirmLabel="Disable server"
        onOpenChange={setDisableConfirmationOpen}
        onConfirm={confirmDisable}
      />
    </>
  )
}
