'use client'

import { useEffect, useRef, useState } from 'react'
import Link from 'next/link'
import { ArrowRight, CheckCircle2, Circle, RefreshCw } from 'lucide-react'

import { authorityIdentity, useBrowserSession } from '@/lib/auth/session'
import { readinessApi, READINESS_CHECKS, type ReadinessCheckName, type ReadinessState } from '@/lib/settings/readiness'
import { Button } from '@/components/ui/button'
import { Progress } from '@/components/ui/progress'
import { SettingsCard } from './SettingsChrome'

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
  const nextCheck = state && READINESS_CHECKS.find((name) => {
    const status = state.checks.find((item) => item.check === name)?.status
    return status !== 'verified' && status !== 'deferred'
  })
  return <SettingsCard title={state?.ready ? 'Your first-use checks passed' : 'Complete your first setup'} description="Follow these steps to get from installation to a working Agent and MCP tool. Verified results are saved so you can return later.">
    <div className="space-y-4 px-4 py-4 sm:px-5">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <p className="text-sm text-aurora-text-muted" role="status">{loading ? 'Reading your saved checks…' : state ? `${completed} ${completed === 1 ? 'check' : 'checks'} verified of ${READINESS_CHECKS.length}${state.checks.some((item) => item.status === 'deferred') ? '; external applications deferred' : ''}` : session.status === 'authenticated' ? 'Your checks could not be verified.' : 'Sign in to read your setup progress.'}</p>
        <Button data-visible-label className="self-start sm:self-auto" size="sm" variant="outline" disabled={loading || saving || session.status !== 'authenticated'} onClick={() => setRefresh((value) => value + 1)}><RefreshCw className="size-3.5" aria-hidden="true" />Refresh checks</Button>
      </div>
      <Progress value={completed / READINESS_CHECKS.length * 100} aria-label="Verified setup checks" aria-valuetext={`${completed} of ${READINESS_CHECKS.length} checks verified`} className="h-1.5" />
      {nextCheck && !loading && <Link href={TASKS[nextCheck].href} className="flex items-center justify-between gap-3 rounded-lg bg-aurora-control-surface px-4 py-3 text-sm text-aurora-text-primary hover:bg-aurora-hover-bg focus-visible:outline-2 focus-visible:outline-aurora-accent-primary"><span><span className="mb-1 block text-xs text-aurora-text-muted">Next step</span><span className="font-medium">{TASKS[nextCheck].label}</span></span><ArrowRight className="size-4 shrink-0 text-aurora-accent-strong" aria-hidden="true" /></Link>}
      {error && <p className="text-sm text-aurora-error" role="alert">{error}</p>}
    </div>
    <ol className="m-0 list-none p-0">
      {READINESS_CHECKS.map((name, index) => {
        const task = TASKS[name]
        const evidence = state?.checks.find((item) => item.check === name)
        const verified = evidence?.status === 'verified'
        const status = loading ? 'Checking' : evidence?.status === 'deferred' ? 'Deferred' : evidence?.status === 'needs_recheck' ? 'Check again' : verified ? 'Verified' : state ? 'Pending' : 'Unverified'
        return <li key={name} className="flex gap-3 border-t border-aurora-border-default/50 px-4 py-4 sm:gap-4 sm:px-5">
          <span className={`mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-full bg-aurora-control-surface ${verified ? 'text-aurora-success' : 'text-aurora-text-muted'}`} aria-hidden="true">{verified ? <CheckCircle2 className="size-4" /> : <span className="text-xs font-medium">{index + 1}</span>}</span>
          <div className="min-w-0 flex-1">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <h3 className="text-sm font-semibold text-aurora-text-primary">{task.label}</h3>
              <span className={`inline-flex items-center gap-1.5 rounded-md bg-aurora-control-surface px-2 py-1 text-xs ${verified ? 'text-aurora-success' : 'text-aurora-text-muted'}`}>{verified ? <CheckCircle2 className="size-3" aria-hidden="true" /> : <Circle className="size-3" aria-hidden="true" />}{status}</span>
            </div>
            <p className="mt-2 text-sm leading-relaxed text-aurora-text-muted">{task.description}</p>
            {verified && evidence?.verified_at != null && <p className="mt-2 text-xs text-aurora-text-muted">Verified {new Date(evidence.verified_at * 1000).toLocaleString()}</p>}
            <div className="mt-3 flex flex-wrap items-center gap-x-4 gap-y-2">
              <Link href={task.href} aria-label={`Open: ${task.label}`} className="inline-flex items-center gap-1.5 text-sm font-medium text-aurora-accent-strong underline-offset-4 hover:underline focus-visible:outline-2 focus-visible:outline-aurora-accent-primary">Open<ArrowRight className="size-3.5" aria-hidden="true" /></Link>
              {name === 'selected_clients' && evidence?.status !== 'verified' && evidence?.status !== 'deferred' && <Button size="sm" variant="outline" className="max-w-full whitespace-normal" disabled={!state || saving || loading} onClick={() => void deferClients()}>Use Labby only for now</Button>}
            </div>
          </div>
        </li>
      })}
    </ol>
    <p className="border-t border-aurora-border-default/50 px-4 py-4 text-xs leading-relaxed text-aurora-text-muted sm:px-5">Configuration changes and expired evidence require another check. Refresh reads saved results and checks your sign-in; it does not run an Agent or invoke a tool.</p>
  </SettingsCard>
}
