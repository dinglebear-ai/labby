'use client'

import Link from 'next/link'
import { useEffect, useState } from 'react'
import { ArrowUpRight, CheckCircle2, CircleAlert, Loader2 } from 'lucide-react'

import { Button } from '@/components/ui/button'
import { SettingsScalarSection } from '@/components/settings/SettingsScalarSection'
import { isAbortError } from '@/lib/api/service-action-client'
import { setupApi, type SettingsSchemaResponse, type SettingsState } from '@/lib/api/setup-client'
import { listAgentModels } from '@/lib/agent-tasks/client'
import { useBrowserSession } from '@/lib/auth/session'
import { fieldsForSection } from '@/lib/settings/schema'

export default function AgentProviderSettingsPage(): React.ReactElement {
  const session = useBrowserSession()
  const principalId = session.status === 'authenticated' ? session.authority?.principalId : undefined
  const [schema, setSchema] = useState<SettingsSchemaResponse>()
  const [settings, setSettings] = useState<SettingsState>()
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string>()
  const [checking, setChecking] = useState(false)
  const [checkResult, setCheckResult] = useState<string>()
  const [checkFailed, setCheckFailed] = useState(false)

  useEffect(() => {
    const controller = new AbortController()
    Promise.all([
      setupApi.settingsSchema(controller.signal),
      setupApi.settingsState('agents', controller.signal),
    ])
      .then(([nextSchema, nextState]) => {
        if (controller.signal.aborted) return
        setSchema(nextSchema)
        setSettings(nextState)
      })
      .catch((reason: unknown) => {
        if (!controller.signal.aborted && !isAbortError(reason)) {
          setError(reason instanceof Error ? reason.message : 'Could not load Agent provider settings.')
        }
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false)
      })
    return () => controller.abort()
  }, [])

  async function checkProvider(): Promise<void> {
    if (!principalId) return
    setChecking(true)
    setCheckResult(undefined)
    setCheckFailed(false)
    try {
      const models = await listAgentModels('personal', principalId)
      setCheckResult(models.length > 0
        ? `The running Labby server reached the Agent provider and listed ${models.length} available model${models.length === 1 ? '' : 's'}. Select one when creating an Agent.`
        : 'The running Labby server reached the Agent provider, but it returned no available models.')
    } catch (reason) {
      setCheckFailed(true)
      setCheckResult(reason instanceof Error ? reason.message : 'Could not check the Agent provider from the Labby server.')
    } finally {
      setChecking(false)
    }
  }

  return <>
    {loading ? <p className="flex items-center gap-2 text-sm text-aurora-text-muted"><Loader2 className="size-4 animate-spin" />Loading Agent provider settings…</p> : null}
    {error ? <p role="alert" className="text-sm text-aurora-error">{error}</p> : null}
    {schema && settings ? <>
      <SettingsScalarSection
        title="Connect an Agent provider"
        description="Connect the API that Labby’s built-in Agents call from this server. Choose OpenAI-compatible API for standard providers, or Phoenix for its session extension. Existing connections retain Phoenix until you choose a protocol. Save the connection, check its models, then run a test Agent."
        section="agents"
        state={settings}
        fields={fieldsForSection(schema.fields, 'agents')}
        onSaved={setSettings}
      />
      <div className="space-y-4 rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-low p-5 sm:p-6">
        <h2 className="text-sm font-semibold text-aurora-text-primary">Check available models</h2>
        <p className="max-w-prose text-sm leading-relaxed text-aurora-text-muted">Labby requests the model list from the provider using the saved connection and any service environment overrides. New Agent runs use this connection immediately; existing runs keep their original connection. A model list confirms access to models; run an Agent to verify generation.</p>
        <Button data-visible-label type="button" size="sm" variant="outline" className="h-9" disabled={checking || !principalId} onClick={() => void checkProvider()}>
          {checking ? <Loader2 className="mr-2 size-4 animate-spin" /> : null}
          Check models
        </Button>
        {checkResult ? <p role={checkFailed ? "alert" : "status"} className="flex items-start gap-2 text-sm leading-relaxed text-aurora-text-primary">{checkFailed ? <CircleAlert aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-aurora-error" /> : <CheckCircle2 aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-aurora-text-muted" />}{checkResult}</p> : null}
      </div>
      <Link href="/agents/" className="inline-flex min-h-9 items-center gap-2 text-sm font-medium text-aurora-accent-strong underline-offset-4 hover:underline">Open Agents to verify the provider<ArrowUpRight aria-hidden="true" className="size-4" /></Link>
    </> : null}
  </>
}
