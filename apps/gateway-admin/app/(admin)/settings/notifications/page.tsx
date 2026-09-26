'use client'

import { useEffect, useState } from 'react'
import { AlertCircle, Bell, Loader2, RefreshCw } from 'lucide-react'

import { Button } from '@/components/ui/button'
import { SettingsCard, SettingsPageHeader } from '@/components/settings/SettingsChrome'
import { SettingsScalarSection } from '@/components/settings/SettingsScalarSection'
import { listNotifications, type LabbyNotification } from '@/lib/api/notifications-client'
import { setupApi, type SettingsSchemaResponse, type SettingsState } from '@/lib/api/setup-client'
import { isAbortError } from '@/lib/api/service-action-client'
import { fieldsForSection } from '@/lib/settings/schema'

export default function NotificationsSettingsPage(): React.ReactElement {
  const [schema, setSchema] = useState<SettingsSchemaResponse>()
  const [settings, setSettings] = useState<SettingsState>()
  const [notifications, setNotifications] = useState<LabbyNotification[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string>()

  function loadNotifications(signal?: AbortSignal): void {
    void listNotifications(signal)
      .then(setNotifications)
      .catch((reason: unknown) => {
        if (!signal?.aborted) setError(reason instanceof Error ? reason.message : 'notification feed unavailable')
      })
  }

  useEffect(() => {
    const controller = new AbortController()
    Promise.all([
      setupApi.settingsSchema(controller.signal),
      setupApi.settingsState('notifications', controller.signal),
      listNotifications(controller.signal),
    ])
      .then(([nextSchema, nextSettings, nextNotifications]) => {
        if (controller.signal.aborted) return
        setSchema(nextSchema)
        setSettings(nextSettings)
        setNotifications(nextNotifications)
      })
      .catch((reason: unknown) => {
        if (controller.signal.aborted || isAbortError(reason)) return
        setError(reason instanceof Error ? reason.message : 'notifications settings unavailable')
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false)
      })
    return () => controller.abort()
  }, [])

  const fields = schema ? fieldsForSection(schema.fields, 'notifications') : []

  return (
    <div className="space-y-4">
      <SettingsPageHeader
        title="Notifications"
        description="Keep ingestion failures visible inside Labby and optionally fan them out through Apprise. Depot polling is read-only and uses Labby’s existing server-held Depot authority."
      />
      {loading ? <p className="flex items-center gap-2 text-xs text-aurora-text-muted"><Loader2 className="size-4 animate-spin" />Loading notification settings…</p> : null}
      {error ? <p role="alert" className="text-xs text-aurora-error">{error}</p> : null}
      {settings ? (
        <SettingsScalarSection
          title="Delivery"
          description="Changes are schema-validated with stale-write protection. APPRISE_TOKEN is a write-only stateful Apprise configuration key; leave it empty to use stateless /notify."
          section="notifications"
          state={settings}
          fields={fields}
          onSaved={setSettings}
        />
      ) : null}
      <SettingsCard
        title="Recent notifications"
        description="A bounded local inbox survives restarts and deduplicates the same failed Depot run."
        action={<Button size="sm" variant="outline" onClick={() => loadNotifications()}><RefreshCw className="size-4" />Refresh</Button>}
      >
        {notifications.length === 0 ? (
          <div className="flex items-center gap-2 p-4 text-xs text-aurora-text-muted"><Bell className="size-4" />No notifications recorded.</div>
        ) : notifications.map((notification) => (
          <div key={notification.id} className="border-t border-aurora-border-subtle p-4 first:border-t-0">
            <div className="flex items-start gap-3">
              <AlertCircle className="mt-0.5 size-4 shrink-0 text-aurora-error" />
              <div className="min-w-0 flex-1">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <strong className="text-sm text-aurora-text-primary">{notification.title}</strong>
                  <span className="text-[11px] text-aurora-text-muted">{new Date(notification.createdAtUnixMs).toLocaleString()}</span>
                </div>
                <p className="mt-1 break-words text-xs leading-5 text-aurora-text-muted">{notification.body}</p>
                <code className="mt-1 block text-[10px] text-aurora-text-muted">{notification.source}</code>
              </div>
            </div>
          </div>
        ))}
      </SettingsCard>
    </div>
  )
}
