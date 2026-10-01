'use client'

import Link from 'next/link'
import { useEffect, useState } from 'react'
import { Loader2 } from 'lucide-react'

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
    try {
      const models = await listAgentModels('personal', principalId)
      setCheckResult(models.length > 0
        ? `The running Labby server reached the Agent provider and listed ${models.length} available model${models.length === 1 ? '' : 's'}. Select one when creating an Agent.`
        : 'The running Labby server reached the Agent provider, but it returned no available models.')
    } catch (reason) {
      setCheckResult(reason instanceof Error ? reason.message : 'Could not check the Agent provider from the Labby server.')
    } finally {
      setChecking(false)
    }
  }

  return <>
    {loading ? <p className="flex items-center gap-2 text-xs text-aurora-text-muted"><Loader2 className="size-4 animate-spin" />Loading Agent provider settings…</p> : null}
    {error ? <p role="alert" className="text-xs text-aurora-error">{error}</p> : null}
    {schema && settings ? <>
      <SettingsScalarSection
        title="Connect an Agent provider"
        description="Labby's built-in Agents use this provider. For a new standard API connection, select OpenAI-compatible API. Select Phoenix only when your provider supports its session extension; existing connections retain Phoenix until you choose a protocol. Save the connection here, check models, then open Agents to choose a model returned by the provider and run a test Agent. Saving a URL or key alone does not verify the connection."
        section="agents"
        state={settings}
        fields={fieldsForSection(schema.fields, 'agents')}
        onSaved={setSettings}
      />
      <div className="space-y-2 rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-low p-4">
        <h2 className="text-sm font-semibold text-aurora-text-primary">Check the running provider connection</h2>
        <p className="text-xs text-aurora-text-muted">Labby requests the model list from the provider using the saved connection and any service environment overrides. New Agent runs use this connection immediately; existing runs keep their original connection. A model list confirms access to models; run an Agent to verify generation.</p>
        <Button type="button" size="sm" variant="outline" disabled={checking || !principalId} onClick={() => void checkProvider()}>
          {checking ? <Loader2 className="mr-2 size-4 animate-spin" /> : null}
          Check models
        </Button>
        {checkResult ? <p role="status" className="text-xs text-aurora-text-primary">{checkResult}</p> : null}
      </div>
      <Link href="/agents/" className="text-xs font-semibold text-aurora-accent-strong underline-offset-2 hover:underline">Open Agents to verify the provider</Link>
    </> : null}
  </>
}
