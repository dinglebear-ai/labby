'use client'

import { useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { SettingsCard, SettingsPageHeader, SettingsRow } from '@/components/settings/SettingsChrome'
import { useBrowserSession } from '@/lib/auth/session'
import { getBrowserSessionContextIdentity } from '@/lib/auth/session-store'
import { isAbortError } from '@/lib/api/service-action-client'
import { tailcatSetupApi, type TailcatConfiguration, type TailcatEnrollment, type TailcatActivation } from '@/lib/api/setup-client'

const shellQuote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`

export function TailcatSetup(): React.ReactElement {
  const session = useBrowserSession()
  const context = getBrowserSessionContextIdentity()
  const projectId = session.status === 'authenticated' ? session.projectId : undefined
  const eligible = session.status === 'authenticated' && session.isConfiguredAdmin === true && Boolean(projectId)
  const [resource, setResource] = useState('')
  const [relay, setRelay] = useState('')
  const [node, setNode] = useState('')
  const [configuration, setConfiguration] = useState<TailcatConfiguration>()
  const [enrollment, setEnrollment] = useState<TailcatEnrollment>()
  const [activation, setActivation] = useState<TailcatActivation>()
  const [credentialId, setCredentialId] = useState('')
  const [error, setError] = useState<string>()
  const [busy, setBusy] = useState(false)
  const lifecycle = useRef({ generation: 0, controller: undefined as AbortController | undefined, busy: false })
  const enrollmentKey = useRef<string | undefined>(undefined)

  useEffect(() => {
    const active = lifecycle.current
    active.generation++
    active.controller?.abort()
    active.busy = false
    enrollmentKey.current = undefined
    setConfiguration(undefined)
    setEnrollment(undefined)
    setActivation(undefined)
    setCredentialId('')
    setError(undefined)
    setBusy(false)
    return () => { active.generation++; active.controller?.abort() }
  }, [context, eligible])

  async function run(task: (signal: AbortSignal) => Promise<() => void>): Promise<void> {
    if (!eligible || lifecycle.current.busy) return
    const issuedUnder = context
    const active = lifecycle.current
    const epoch = ++active.generation
    const controller = new AbortController()
    active.controller?.abort()
    active.controller = controller
    active.busy = true
    const requestSignal = AbortSignal.any([controller.signal, AbortSignal.timeout(30000)])
    const current = () => !controller.signal.aborted && epoch === active.generation
      && issuedUnder === getBrowserSessionContextIdentity()
    setBusy(true)
    setError(undefined)
    try {
      const publish = await task(requestSignal)
      if (current()) publish()
    } catch (reason) {
      if (current()) {
        if (requestSignal.aborted && !controller.signal.aborted) setError('The native request timed out. Retry the same step; enrollment retains its operation key.')
        else if (!isAbortError(reason)) setError(reason instanceof Error ? reason.message : 'Tailcat setup failed. Retry the same step.')
      }
    } finally { if (current()) { active.busy = false; setBusy(false) } }
  }

  function configure(dryRun: boolean): void {
    void run(async signal => {
      const result = await tailcatSetupApi.tailcatConfigure({ project_id: projectId!, public_resource: resource,
        derp_map_url: relay, node_path: node, dry_run: dryRun }, signal)
      return () => {
        setConfiguration(result)
        if (!result.dry_run) { enrollmentKey.current = undefined; setEnrollment(undefined); setActivation(undefined); setCredentialId('') }
      }
    })
  }

  const fieldsReady = resource.trim() !== '' && relay.trim() !== '' && node.trim() !== ''
  function edit(setValue: (value: string) => void, value: string): void {
    setValue(value)
    setConfiguration(undefined)
    setEnrollment(undefined)
    setActivation(undefined)
    setCredentialId('')
  }
  return <div className="space-y-6">
    <SettingsPageHeader title="Tailcat" description="Connect the dashboard to this native Labby gateway through browser networking." />
    {!eligible ? <p role="status" className="text-sm text-aurora-text-muted">Sign in as the configured operator and select a project. Labby verifies project ownership before making changes.</p> : null}
    <SettingsCard title="1. Prepare the restricted connection" description={projectId ? `Selected project: ${projectId}. OAuth and verified release companions must already be configured on the native host.` : 'Select a project to begin.'}>
      <SettingsRow label="Public MCP resource URL" htmlFor="tailcat-resource" layout="stacked"
        description="The HTTPS URL of this project's protected native gateway route."
        control={<Input id="tailcat-resource" value={resource} disabled={!eligible || busy} onChange={event => edit(setResource, event.target.value)} placeholder="https://your-host.example/sandbox" />} />
      <SettingsRow label="DERP map URL" htmlFor="tailcat-relay" layout="stacked"
        description="Use the same HTTPS relay map configured for the dashboard's Tailcat assets."
        control={<Input id="tailcat-relay" value={relay} disabled={!eligible || busy} onChange={event => edit(setRelay, event.target.value)} placeholder="https://your-relay.example/derpmap.json" />} />
      <SettingsRow label="Node executable on this host" htmlFor="tailcat-node" layout="stacked"
        description="Absolute path to the installed Node executable that runs the private Microsandbox adapter."
        control={<Input id="tailcat-node" value={node} disabled={!eligible || busy} onChange={event => edit(setNode, event.target.value)} placeholder="/absolute/path/to/node" />} />
      <SettingsRow control={<div className="flex flex-wrap gap-2">
        <Button variant="outline" disabled={!eligible || busy || !fieldsReady} onClick={() => configure(true)}>Preview configuration</Button>
        <Button disabled={!eligible || busy || !fieldsReady} onClick={() => configure(false)}>Save configuration</Button>
      </div>} />
      {configuration ? <SettingsRow layout="stacked" label={configuration.dry_run ? 'Preview only' : 'Configuration saved'} description={<>
        Exact project loadout: <code>{configuration.loadout}</code>. {configuration.dry_run ? 'No configuration was written.' : 'Assign this exact loadout using the existing authorized project assignment action, then reload the gateway before enrollment.'}
        {configuration.restart_required && !configuration.dry_run ? ' A native gateway restart is required.' : ''}
      </>} /> : null}
    </SettingsCard>
    <SettingsCard title="2. Enroll the project credential" description="After the native gateway restarts with the exact assigned loadout, continue here without saving configuration again. Labby checks the live project policy and publishes the credential into a private native file. Retrying within this page uses the same operation.">
      <SettingsRow control={<Button disabled={!eligible || busy} onClick={() => {
        enrollmentKey.current ??= crypto.randomUUID()
        const key = enrollmentKey.current
        void run(async signal => {
          const result = await tailcatSetupApi.tailcatEnroll(projectId!, key, signal)
          return () => { setEnrollment(result); setCredentialId(result.credential_id); setActivation(undefined) }
        })
      }}>Enroll credential</Button>} />
      {enrollment ? <SettingsRow layout="stacked" label="Credential enrolled" description={<>
        Private host file: <code className="break-all">{enrollment.credential_file}</code>.
        {' '}Expires {new Date(enrollment.expires_at * 1000).toLocaleString()}.
        <p className="mt-2">Use this file for local approval: <code className="break-all">labby tailcat pair --credential-file {shellQuote(enrollment.credential_file)}</code>. The dashboard supplies the remaining pairing arguments.</p>
      </>} /> : null}
    </SettingsCard>
    <SettingsCard title="3. Enable dashboard networking" description="The server rechecks the enrolled credential, OAuth, project policy and installed companions before enabling the controller.">
      <SettingsRow label="Enrolled credential ID" htmlFor="tailcat-credential-id" layout="stacked"
        description="To resume after a page refresh, enter the public credential ID returned by enrollment. This is an identifier, not the credential bearer."
        control={<Input id="tailcat-credential-id" value={credentialId} disabled={!eligible || busy} onChange={event => { setCredentialId(event.target.value); setActivation(undefined) }} />} />
      <SettingsRow control={<Button disabled={!eligible || busy || !credentialId.trim()} onClick={() => void run(async signal => {
        const result = await tailcatSetupApi.tailcatEnable(projectId!, credentialId.trim(), signal)
        return () => setActivation(result)
      })}>Enable Tailcat</Button>} />
      {activation ? <SettingsRow layout="stacked" label={activation.enabled ? 'Controller enabled in configuration' : 'Controller remains disabled'}
        description={activation.restart_required ? 'Restart the native gateway, then pair from the dashboard and approve locally.' : 'Use the native controller status to verify the running connection.'} /> : null}
    </SettingsCard>
    {busy ? <p role="status" className="text-sm text-aurora-text-muted">Checking the native gateway…</p> : null}
    {error ? <p role="alert" className="text-sm text-aurora-error">{error}</p> : null}
  </div>
}
