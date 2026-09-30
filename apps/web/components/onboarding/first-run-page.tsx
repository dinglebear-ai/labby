'use client'

import { useEffect, useRef, useState, useSyncExternalStore } from 'react'
import Link from 'next/link'
import { ArrowRight, Bot, CheckCircle2, Circle, Compass, Loader2, Plug, RefreshCw } from 'lucide-react'
import { AppHeader } from '@/components/app-header'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { ConsoleHero } from '@/components/console/console-hero'
import { DashboardPanel } from '@/components/dashboard/panel'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { firstRunApi, type FirstRunState, type VerifiedFirstRunProvider } from '@/lib/api/setup-client'
import { listArtifacts, type DiscoveryPage } from '@/lib/api/depot-client'
import { createAgent, listAgents, runAgent, type AgentRunResult, type AgentView } from '@/lib/agent-tasks/client'
import { getBrowserSessionContextIdentity, getSessionAuthority, subscribeToBrowserSession } from '@/lib/auth/session-store'
import type { Gateway } from '@/lib/types/gateway'
import { ConnectMcpForm } from './connect-mcp-form'
import { agentRunVerified, discoveryVerified, firstUseSummary, mcpConnectionVerified } from './readiness-model'

const MOCK_MODE = process.env.NEXT_PUBLIC_MOCK_DATA === 'true'
const serverSnapshot = () => ''
const messageOf = (error: unknown) => error instanceof Error ? error.message : 'The check did not complete. Retry when the gateway is reachable.'

export function FirstRunPage() {
  const context = useSyncExternalStore(subscribeToBrowserSession, getBrowserSessionContextIdentity, serverSnapshot)
  // Authority changes discard all in-memory evidence, credentials, and requests.
  return <FirstRunJourney key={context} />
}

function FirstRunJourney() {
  const [state, setState] = useState<FirstRunState | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [reload, setReload] = useState(0)
  const [provider, setProvider] = useState<VerifiedFirstRunProvider | null>(null)
  const [agentVerified, setAgentVerified] = useState(false)
  const [catalog, setCatalog] = useState<DiscoveryPage | null>(null)
  const [gateway, setGateway] = useState<Gateway | null>(null)
  const summary = firstUseSummary(Boolean(provider), agentVerified, discoveryVerified(catalog, MOCK_MODE), mcpConnectionVerified(gateway))

  useEffect(() => {
    const controller = new AbortController()
    setLoadError(null)
    void firstRunApi.state(controller.signal).then(value => {
      if (controller.signal.aborted) return
      if (value.schema_version !== 1 || !value.provider) throw new Error('The gateway does not support this first-run guide yet. Update Labby and retry.')
      setState(value)
    }).catch(error => { if (!controller.signal.aborted) setLoadError(messageOf(error)) })
    return () => controller.abort()
  }, [reload])

  return <>
    <AppHeader breadcrumbs={[{ label: 'Labby' }, { label: 'Get started' }]} />
    <div className={AURORA_PAGE_SHELL}><div className={`${AURORA_PAGE_FRAME} space-y-5`}>
      <ConsoleHero eyebrow="First run" title="From installed to useful" description="Connect your provider, run your first Agent, find a server, and check its MCP connection. No manual .env edits on this path." stats={[{ label: 'Checks verified', value: `${summary.verified} / ${summary.total}`, icon: <CheckCircle2 size={12} /> }]} actions={<Button asChild variant="outline"><Link href="/settings">Advanced settings<ArrowRight aria-hidden="true" className="size-4" /></Link></Button>} />
      <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4" aria-label="First-run progress">
        <Check label="Provider responds" verified={Boolean(provider)} />
        <Check label="Agent completed a run" verified={agentVerified} />
        <Check label="Discover returns results" verified={discoveryVerified(catalog, MOCK_MODE)} />
        <Check label="MCP tools are available" verified={mcpConnectionVerified(gateway)} />
      </div>
      {MOCK_MODE ? <p role="alert" className="rounded-aurora-2 border border-aurora-border-default p-4 text-sm text-aurora-warn">Preview data is enabled. Catalog fixtures cannot qualify this installation as ready.</p> : null}
      {loadError ? <div role="alert" className="space-y-3 rounded-aurora-2 border border-aurora-border-default p-4 text-sm"><p>{loadError}</p><Button variant="outline" onClick={() => setReload(value => value + 1)}><RefreshCw aria-hidden="true" className="size-4" />Retry setup status</Button></div> : null}
      {!state && !loadError ? <p role="status" className="flex items-center gap-2 text-sm text-aurora-text-muted"><Loader2 aria-hidden="true" className="size-4 animate-spin" />Reading this installation…</p> : null}
      {state ? <div className="grid items-start gap-5 xl:grid-cols-2">
        <DashboardPanel title="1. Connect your Agent provider"><ProviderStep state={state} onVerified={value => { setProvider(value); setAgentVerified(false) }} /></DashboardPanel>
        <DashboardPanel title="2. Create and test your Agent"><AgentStep provider={provider} onVerified={setAgentVerified} /></DashboardPanel>
        <DashboardPanel title="3. Find your first MCP server"><DiscoverStep page={catalog} onResults={setCatalog} /></DashboardPanel>
        <DashboardPanel title="4. Connect and check the server"><ConnectMcpForm onConnected={setGateway} /></DashboardPanel>
      </div> : null}
      <DashboardPanel title={summary.coreReady ? 'Core connection checks passed' : 'What counts as ready?'}>
        <div className="space-y-3 text-sm text-aurora-text-muted">
          <p>The checks above report observed results, not installation promises. A saved definition, copied client configuration, or Library import is not proof that a tool works.</p>
          <p>After connecting a server, connect your chosen client from Gateway and make a harmless tool call. Downstream client registration and that first tool call are not verified by this guide yet.</p>
          <p>Credentials stay out of browser storage. Reloading this page re-reads configuration and clears verification badges; it does not erase your provider, Agent, or server.</p>
          <div className="flex flex-wrap gap-3"><Button asChild variant="outline"><Link href="/agents"><Bot aria-hidden="true" className="size-4" />Agents</Link></Button><Button asChild variant="outline"><Link href="/gateway"><Plug aria-hidden="true" className="size-4" />Gateway and clients</Link></Button><Button asChild variant="outline"><Link href="/tools">Inspect tools<ArrowRight aria-hidden="true" className="size-4" /></Link></Button></div>
        </div>
      </DashboardPanel>
    </div></div>
  </>
}

function Check({ label, verified }: { label: string; verified: boolean }) {
  const Icon = verified ? CheckCircle2 : Circle
  return <div className="flex items-center gap-3 rounded-aurora-2 border border-aurora-border-default bg-aurora-panel-medium p-4"><Icon aria-hidden="true" className={`size-5 shrink-0 ${verified ? 'text-aurora-success' : 'text-aurora-text-muted'}`} /><div className="min-w-0 text-sm"><p className="font-semibold">{label}</p><p className="text-xs text-aurora-text-muted">{verified ? 'Verified this session' : 'Not verified'}</p></div></div>
}

function ProviderStep({ state, onVerified }: { state: FirstRunState; onVerified: (provider: VerifiedFirstRunProvider | null) => void }) {
  const [baseUrl, setBaseUrl] = useState(state.provider.base_url ?? '')
  const [apiKey, setApiKey] = useState('')
  const [configured, setConfigured] = useState(state.provider.configured)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [verified, setVerified] = useState(false)
  const pending = useRef(false)

  const verify = async () => {
    if (pending.current) return
    pending.current = true; setBusy(true); setError(null); setVerified(false); onVerified(null)
    try {
      const result = configured ? await firstRunApi.models() : await firstRunApi.configureProvider(baseUrl, apiKey)
      if (!result.provider.configured || result.models.length === 0) throw new Error('The provider did not advertise a usable model.')
      setConfigured(true); setVerified(true); setBaseUrl(result.provider.base_url ?? baseUrl)
      onVerified(result)
    } catch (failure) { setError(messageOf(failure)); setVerified(false) }
    finally { pending.current = false; setBusy(false); setApiKey('') }
  }

  return <form onSubmit={event => { event.preventDefault(); void verify() }} className="space-y-4">
    <p className="text-sm text-aurora-text-muted">Connect an OpenAI-compatible service. You need an existing provider account or a running local model server.</p>
    <div className="space-y-2"><Label htmlFor="first-provider-url">Provider base URL</Label><Input id="first-provider-url" value={baseUrl} onChange={event => setBaseUrl(event.target.value)} placeholder="https://provider.example/v1" type="url" disabled={configured || busy} required /><p className="text-xs text-aurora-text-muted">Reachable from the Labby gateway host. For a remote gateway, localhost means that remote host, not this browser.</p></div>
    {!configured ? <div className="space-y-2"><Label htmlFor="first-provider-key">Provider API key (optional for a local service)</Label><Input id="first-provider-key" type="password" value={apiKey} onChange={event => setApiKey(event.target.value)} autoComplete="new-password" disabled={busy} /><p className="text-xs text-aurora-text-muted">Stored through Labby’s protected configuration writer. HTTPS is required for credentials outside loopback.</p></div> : <p className="text-xs text-aurora-text-muted">Existing provider preserved. {state.provider.externally_managed ? 'Its settings are managed by the service environment.' : 'Use Settings to change its configuration.'}</p>}
    {!configured ? <p className="text-xs text-aurora-text-muted">First-provider configuration requires the trusted local setup connection. This guide does not bypass remote setup authorization.</p> : null}
    {error ? <p role="alert" className="text-sm text-aurora-error">{error}</p> : null}
    {verified ? <p role="status" className="text-sm text-aurora-success">Provider responded with selectable models. New Agents can use it without a daemon restart.</p> : null}
    <Button type="submit" disabled={busy || (!configured && !baseUrl.trim())}>{busy ? <Loader2 aria-hidden="true" className="size-4 animate-spin" /> : <CheckCircle2 aria-hidden="true" className="size-4" />}{busy ? 'Checking provider…' : configured ? 'Verify provider and load models' : 'Connect provider and load models'}</Button>
  </form>
}

function AgentStep({ provider, onVerified }: { provider: VerifiedFirstRunProvider | null; onVerified: (value: boolean) => void }) {
  const [model, setModel] = useState('')
  const [existing, setExisting] = useState<AgentView[]>([])
  const [selectedId, setSelectedId] = useState('')
  const [created, setCreated] = useState<AgentView | null>(null)
  const [result, setResult] = useState<AgentRunResult | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const pending = useRef(false)
  const principal = getSessionAuthority()?.principalId
  const models = provider?.models ?? []
  const selectedModel = models.includes(model) ? model : models[0] ?? ''

  useEffect(() => {
    const controller = new AbortController()
    void listAgents(controller.signal).then(agents => { if (!controller.signal.aborted) setExisting(agents.filter(agent => agent.owner_kind === 'personal' && agent.owner_id === principal && agent.state === 'active')) }).catch(failure => { if (!controller.signal.aborted) setError(messageOf(failure)) })
    return () => controller.abort()
  }, [principal])

  const run = async () => {
    if (pending.current || !provider || !selectedModel || !principal) return
    pending.current = true; setBusy(true); setError(null); setResult(null); onVerified(false)
    try {
      let agent = created ?? existing.find(item => item.agent_id === selectedId) ?? null
      if (!agent) {
        agent = await createAgent({ agentId: `starter-${crypto.randomUUID().slice(0, 8)}`, ownerKind: 'personal', ownerId: principal,
          model: selectedModel, instructions: 'You are a helpful personal assistant. Follow the user request. This first-run check needs no tools or external actions.' })
        setCreated(agent)
      }
      const runResult = await runAgent(agent.agent_id, 'Reply with a short greeting to confirm that this Agent is responding. Do not use tools.')
      if (!agentRunVerified(agent, runResult)) throw new Error('The Agent did not return a matching completed execution receipt.')
      setResult(runResult)
      onVerified(true)
    } catch (failure) { setError(messageOf(failure)) }
    finally { pending.current = false; setBusy(false) }
  }

  return <div className="space-y-4">
    <p className="text-sm text-aurora-text-muted">Your personal Agent belongs to your signed-in identity. No owner ID to find or copy.</p>
    {!provider ? <p className="rounded-aurora-1 bg-aurora-control-surface p-3 text-sm text-aurora-text-muted">Connect and verify your provider first.</p> : null}
    {existing.length > 0 && !created ? <div className="space-y-2"><Label htmlFor="first-existing-agent">Agent</Label><select id="first-existing-agent" className="h-10 w-full rounded-md border border-aurora-border-default bg-aurora-control-surface px-3 text-sm" value={selectedId} disabled={busy} onChange={event => { setSelectedId(event.target.value); setResult(null); onVerified(false) }}><option value="">Create a new starter Agent</option>{existing.map(agent => <option key={agent.agent_id} value={agent.agent_id}>{agent.agent_id}</option>)}</select></div> : null}
    {!selectedId && !created ? <div className="space-y-2"><Label htmlFor="first-agent-model">Model advertised by your provider</Label><select id="first-agent-model" className="h-10 w-full rounded-md border border-aurora-border-default bg-aurora-control-surface px-3 text-sm" value={selectedModel} disabled={!provider || busy} onChange={event => setModel(event.target.value)}>{models.length ? models.map(id => <option key={id} value={id}>{id}</option>) : <option value="">Verify provider to load models</option>}</select></div> : null}
    {created ? <p className="text-sm">Created <strong>{created.agent_id}</strong>. Retrying checks this Agent again without creating duplicates.</p> : null}
    <p className="text-xs text-aurora-text-muted">This sends one short model request and may use provider credit. It does not request a tool call.</p>
    {!principal ? <p className="text-xs text-aurora-warn">A signed-in personal identity is required.</p> : null}
    {error ? <p role="alert" className="text-sm text-aurora-error">{error}</p> : null}
    {provider && result?.status === 'completed' ? <div role="status" className="space-y-2 rounded-aurora-1 border border-aurora-border-default p-3"><p className="text-sm text-aurora-success">Agent execution completed.</p><p className="max-h-32 overflow-auto whitespace-pre-wrap break-words text-sm">{result.output?.slice(0, 1000) || 'A durable output receipt was returned.'}</p></div> : null}
    <Button disabled={busy || !provider || !selectedModel || !principal} onClick={() => void run()}>{busy ? <Loader2 aria-hidden="true" className="size-4 animate-spin" /> : <Bot aria-hidden="true" className="size-4" />}{busy ? 'Running Agent check…' : created || selectedId ? 'Test this Agent' : 'Create and test my Agent'}</Button>
  </div>
}

function DiscoverStep({ page, onResults }: { page: DiscoveryPage | null; onResults: (page: DiscoveryPage | null) => void }) {
  const [query, setQuery] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const pending = useRef(false)
  const search = async () => {
    if (pending.current) return
    pending.current = true; setBusy(true); setError(null); onResults(null)
    try {
      const result = await listArtifacts({ query: query.trim(), kind: 'mcp-server', limit: 5 })
      onResults(result)
      if (result.state === 'all_failed') throw new Error('Every catalog provider failed. This is a connectivity or authorization problem, not an empty search.')
      if (result.state === 'all_disabled') throw new Error('No catalog providers are enabled. Enable a provider in Settings.')
    } catch (failure) { setError(messageOf(failure)) }
    finally { pending.current = false; setBusy(false) }
  }
  return <form onSubmit={event => { event.preventDefault(); void search() }} className="space-y-4">
    <p className="text-sm text-aurora-text-muted">Search real catalog results. Open a result to review its publisher, revision, and connection instructions before adding a server.</p>
    <div className="space-y-2"><Label htmlFor="first-discover-query">Search MCP servers</Label><Input id="first-discover-query" value={query} onChange={event => setQuery(event.target.value)} placeholder="Search by task or server name" maxLength={200} /></div>
    <Button type="submit" disabled={busy || (query.trim().length > 0 && query.trim().length < 3)}>{busy ? <Loader2 aria-hidden="true" className="size-4 animate-spin" /> : <Compass aria-hidden="true" className="size-4" />}{busy ? 'Searching catalog…' : 'Search Discover'}</Button>
    {error ? <p role="alert" className="text-sm text-aurora-error">{error}</p> : null}
    {page?.failures.length ? <p className="text-xs text-aurora-warn">Partial catalog coverage: {page.failures.map(failure => `${failure.providerId} (${failure.kind})`).join(', ')}. Available results are shown below.</p> : null}
    {page && !page.items.length && !error ? <p role="status" className="text-sm text-aurora-text-muted">No results for this search. Try a different term; this does not complete catalog verification.</p> : null}
    <div className="space-y-2">{page?.items.map(artifact => <Link key={`${artifact.providerId}:${artifact.artifactId}`} href={`/depot?${new URLSearchParams({ artifactProvider: artifact.providerId, artifact: artifact.artifactId })}`} className="block rounded-aurora-1 border border-aurora-border-default p-3 text-sm transition-colors hover:bg-aurora-hover-bg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary"><p className="font-semibold">{artifact.title ?? artifact.name ?? artifact.descriptor?.name ?? artifact.artifactId}</p><p className="line-clamp-2 text-xs text-aurora-text-muted">{artifact.description ?? artifact.descriptor?.description ?? artifact.providerId}</p></Link>)}</div>
  </form>
}
