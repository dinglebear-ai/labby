'use client'

import { useEffect, useRef, useState } from 'react'
import Link from 'next/link'
import { ArrowRight, CheckCircle2, Circle, RefreshCw } from 'lucide-react'

import { authorityIdentity, useBrowserSession } from '@/lib/auth/session'
import { readinessApi, READINESS_CHECKS, type ReadinessCheckName, type ReadinessState } from '@/lib/settings/readiness'
import { Button } from '@/components/ui/button'
import { SettingsCard, SettingsRow } from './SettingsChrome'

const TASKS: Record<ReadinessCheckName, { label: string; description: string; href: string }> = {
  gateway_authenticated: { label: 'Labby is running and authenticated', description: 'This server accepted your current identity and found your active Labby account.', href: '/settings/doctor/' },
  agent_provider: { label: 'Connect your Agent provider', description: 'Test the provider from the Labby server and retrieve its available models. Saving a credential alone leaves this step pending.', href: '/settings/agents/' },
  agent_run: { label: 'Create and test your starter Agent', description: 'Run an Agent using your connected provider and confirm it returns a response. This test may incur your provider’s usage charges.', href: '/agents/' },
  selected_clients: { label: 'Connect your selected applications', description: 'Register Labby with the applications on your computer, finish their login, and verify their connection. Browser settings cannot configure applications on another computer.', href: '/settings/surfaces/' },
  catalog_search: { label: 'Search Discover', description: 'Read the configured catalog and obtain real search results. Access denied and a catalog that is still indexing remain incomplete.', href: '/depot/' },
  mcp_tool_call: { label: 'Add an MCP server and use a tool', description: 'Authorize a supported server, check its exposed tools, and approve a safe first tool call. Adding an artifact to Library does not complete this step.', href: '/gateways/' },
}

export function FirstUseReadiness(): React.ReactElement {
  const session = useBrowserSession()
  const identity = session.status === 'authenticated' ? `${session.user.sub}:${authorityIdentity(session.authority)}` : session.status
  const [data, setData] = useState<{ identity: string; state: ReadinessState } | null>(null)
  const state = data?.identity === identity ? data.state : null
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const mutation = useRef<AbortController | null>(null)
  const [refresh, setRefresh] = useState(0)

  useEffect(() => {
    const controller = new AbortController()
    setSaving(false)
    setData(null)
    setError(null)
    if (session.status !== 'authenticated') {
      setLoading(false)
      return () => { controller.abort(); mutation.current?.abort() }
    }
    setLoading(true)
    void readinessApi.state(controller.signal).then((next) => {
      if (!controller.signal.aborted) setData({ identity, state: next })
    }, (cause: unknown) => {
      if (!controller.signal.aborted) setError(cause instanceof Error ? cause.message : 'First-use status could not be read. Refresh to try again.')
    }).finally(() => {
      if (!controller.signal.aborted) setLoading(false)
    })
    return () => { controller.abort(); mutation.current?.abort() }
  }, [identity, refresh, session.status])

  async function deferClients() {
    const controller = new AbortController()
    mutation.current?.abort()
    mutation.current = controller
    setSaving(true)
    setError(null)
    try {
      const next = await readinessApi.deferClients(controller.signal)
      if (!controller.signal.aborted) setData({ identity, state: next })
    } catch (cause) {
      if (!controller.signal.aborted) setError(cause instanceof Error ? cause.message : 'Your application choice could not be saved.')
    } finally {
      if (!controller.signal.aborted) setSaving(false)
    }
  }

  const completed = state?.checks.filter((item) => item.status === 'verified').length ?? 0
  return <SettingsCard title={state?.ready ? 'Your first-use checks passed' : 'Complete your first setup'} description="Labby saves verified results so you can leave and resume. Configuration changes and expired evidence require another check. Installation alone leaves the remaining steps pending.">
    <div className="flex items-center justify-between gap-3 px-4 py-3">
      <p className="text-xs text-aurora-text-muted" role="status">{loading ? 'Reading your saved checks…' : state ? `${completed} checks verified${state.checks.some((item) => item.status === 'deferred') ? '; external applications deferred' : ` of ${READINESS_CHECKS.length}`}` : session.status === 'authenticated' ? 'Your checks could not be verified.' : 'Sign in to read your setup progress.'}</p>
      <Button size="sm" variant="outline" disabled={loading || saving || session.status !== 'authenticated'} onClick={() => setRefresh((value) => value + 1)}><RefreshCw className="size-3.5" aria-hidden="true" />Refresh checks</Button>
    </div>
    {error && <p className="px-4 pb-3 text-xs text-aurora-error" role="alert">{error}</p>}
    {READINESS_CHECKS.map((name) => {
      const task = TASKS[name]
      const evidence = state?.checks.find((item) => item.check === name)
      const verified = evidence?.status === 'verified'
      const status = loading ? 'Checking' : evidence?.status === 'deferred' ? 'Deferred' : evidence?.status === 'needs_recheck' ? 'Check again' : verified ? 'Verified' : state ? 'Pending' : 'Unverified'
      return <SettingsRow key={name} label={task.label} description={<>{task.description}{verified && evidence?.verified_at !== null && <span className="mt-1 block">Verified {new Date((evidence?.verified_at ?? 0) * 1000).toLocaleString()}.</span>}</>} control={<div className="flex flex-wrap items-center justify-end gap-2"><span className="inline-flex items-center gap-1.5 text-xs text-aurora-text-muted">{verified ? <CheckCircle2 className="size-3.5 text-aurora-success" aria-hidden="true" /> : <Circle className="size-3.5" aria-hidden="true" />}{status}</span>{name === 'selected_clients' && evidence?.status !== 'verified' && evidence?.status !== 'deferred' && <Button size="sm" variant="outline" disabled={!state || saving || loading} onClick={() => void deferClients()}>Use Labby only for now</Button>}<Link href={task.href} className="inline-flex items-center gap-1 whitespace-nowrap text-xs font-semibold text-aurora-accent-strong underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-aurora-accent-primary">Open<ArrowRight className="size-3" aria-hidden="true" /></Link></div>} />
    })}
    <p className="px-4 py-3 text-xs text-aurora-text-muted">These are recent verified results, with the time shown for each step. Refreshing reads the saved evidence and verifies your current sign-in; it does not run an Agent or invoke a tool.</p>
  </SettingsCard>
}
