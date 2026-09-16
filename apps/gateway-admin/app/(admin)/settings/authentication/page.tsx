'use client'

import { useEffect, useState } from 'react'
import { Loader2, ShieldAlert } from 'lucide-react'

import { AllowedUsersPanel } from '@/components/allowed-users-panel'
import { SettingsScalarSection } from '@/components/settings/SettingsScalarSection'
import { isAbortError } from '@/lib/api/service-action-client'
import { setupApi, type SettingsSchemaResponse, type SettingsState } from '@/lib/api/setup-client'
import { useBrowserSession } from '@/lib/auth/session'
import { fieldsForSection } from '@/lib/settings/schema'

/**
 * Authentication panel — the administrator list (env-backed, operator-only
 * writes enforced by the server) above the sign-in allowlist.
 *
 * Both controls are accepted by the server only from a configured admin's
 * browser session (`is_configured_admin`), never from platform
 * administration alone, so the page offers them on that same signal.
 */
export default function AuthenticationPage(): React.ReactElement {
  const session = useBrowserSession()
  const sessionLoading = session.status === 'loading'
  const isConfiguredAdmin =
    session.status === 'authenticated' && session.isConfiguredAdmin === true
  const [schema, setSchema] = useState<SettingsSchemaResponse | undefined>()
  const [settings, setSettings] = useState<SettingsState | undefined>()
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | undefined>()

  useEffect(() => {
    if (!isConfiguredAdmin) {
      setLoading(false)
      return
    }
    const controller = new AbortController()
    Promise.all([
      setupApi.settingsSchema(controller.signal),
      setupApi.settingsState('authentication', controller.signal),
    ])
      .then(([schemaResponse, stateResponse]) => {
        if (controller.signal.aborted) return
        setSchema(schemaResponse)
        setSettings(stateResponse)
      })
      .catch((err: unknown) => {
        if (controller.signal.aborted || isAbortError(err)) return
        setError(`Authentication settings: ${err instanceof Error ? err.message : 'load failed'}`)
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false)
      })
    return () => controller.abort()
  }, [isConfiguredAdmin])

  const fields = schema ? fieldsForSection(schema.fields, 'authentication') : []

  if (sessionLoading) {
    return (
      <p className="flex items-center gap-2 text-[11.5px] text-aurora-text-muted">
        <Loader2 className="h-4 w-4 animate-spin" /> Checking permission…
      </p>
    )
  }
  if (!isConfiguredAdmin) {
    return (
      <div role="alert" className="flex gap-2 text-[11.5px] text-aurora-error">
        <ShieldAlert className="size-4 shrink-0" />
        <span>
          Only a configured administrator (an email listed in LABBY_AUTH_ADMIN_EMAIL) can manage
          administrators and allowed users. Platform administration alone does not grant this.
        </span>
      </div>
    )
  }

  return (
    <>
      <h2 className="sr-only">Authentication settings</h2>
      {loading ? (
        <div className="flex items-center gap-2 text-[11.5px] text-aurora-text-muted">
          <Loader2 className="h-4 w-4 animate-spin" /> loading authentication settings
        </div>
      ) : null}
      {error ? <p role="alert" className="text-[11.5px] text-destructive">{error}</p> : null}
      {settings ? (
        <SettingsScalarSection
          title="Administrators"
          description="Everyone listed here gets full admin access after signing in. Changes apply after a restart. Only the operator (a listed administrator or a local operator credential) can save them."
          section="authentication"
          state={settings}
          fields={fields}
          onSaved={setSettings}
        />
      ) : null}
      <AllowedUsersPanel />
    </>
  )
}
