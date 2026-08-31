'use client'

import { FormEvent, useEffect, useRef, useState } from 'react'
import { SearchCode, ShieldCheck, TriangleAlert } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { subscribeToBrowserSession } from '@/lib/auth/session-store'
import { describeCodeModeTool, searchCodeModeTools, type ToolDescription, type ToolSearchHit } from '@/lib/api/tool-browser-client'
import { GatewayApiError } from '@/lib/api/gateway-client-core'
import { useGatewayCodeModeConfig } from '@/lib/hooks/use-gateways'

type BrowserError = { message: string; status?: number; requestId?: string; retry?: () => void }

function isAbortError(error: unknown) {
  return error instanceof DOMException && error.name === 'AbortError'
}

export function toolBrowserError(error: unknown, fallback: string): BrowserError {
  if (error instanceof GatewayApiError) {
    if (error.status === 401) return { message: 'Sign in to search tools.', status: 401, requestId: error.requestId }
    if (error.status === 403) return { message: 'Administrator access is required.', status: 403, requestId: error.requestId }
    if (error.status >= 500) return { message: 'Tools are temporarily unavailable.', status: error.status, requestId: error.requestId }
    return { message: error.message, status: error.status, requestId: error.requestId }
  }
  return { message: error instanceof Error ? error.message : fallback }
}

export interface ResultsSummaryState {
  total: number
  shown: number
  hasError: boolean
  /** The query that produced the current results, or `null` before any search. */
  executedQuery: string | null
  loading: boolean
  /** `codeModeConfig?.enabled` — `undefined` when unknown. */
  codeModeEnabled: boolean | undefined
  codeModeConfigMissing: boolean
  codeModeConfigFailed: boolean
}

/**
 * Pick the one-line label under the search box.
 *
 * Exported and pure because this is the whole point of the fix: a completed
 * "Browse all" that finds nothing used to render the same placeholder as
 * before any search ran, so nothing told the operator the request had
 * happened — which is exactly what a disabled Code Mode looks like, since the
 * search endpoint reports 0 tools rather than why. The rules are subtle enough
 * to deserve direct tests, and they cannot be driven through the component:
 * the unit-test DOM does not implement React's synthetic change events, so a
 * simulated keystroke silently does nothing and any test written that way
 * passes against the bug as readily as against the fix.
 */
export function resultsSummary(state: ResultsSummaryState): string {
  // Keep the count when a *detail* fetch fails: `loadDetail` sets `error`
  // without touching results, and blanking the label while result cards are
  // still on screen just looks broken.
  if (state.total > 0) {
    return `${state.total} matches${state.shown < state.total ? ` · showing ${state.shown}` : ''}`
  }
  if (state.hasError) return ''
  if (state.executedQuery === null || state.loading) {
    return 'Search, or browse the live catalog without a query'
  }
  // Branch on the query that produced these results, not the live input —
  // otherwise clearing the box after a failed search silently upgrades
  // "no matches for zzz" into a claim about every connected server.
  if (state.executedQuery.trim()) return 'No matching tools'
  if (state.codeModeEnabled === false) {
    return 'Code Mode is disabled, so this catalog is empty. Enable it from Gateway.'
  }
  // Check the failure before the missing check, not inside it. SWR serves the
  // last-good `data` when a revalidation fails, so a stale-but-present config
  // with a live fetch error would otherwise fall through to the confident
  // gateway-wide claim below while we are in fact flying blind.
  if (state.codeModeConfigFailed) {
    return 'No tools returned, and the Code Mode setting could not be read to explain why.'
  }
  if (state.codeModeConfigMissing || state.codeModeEnabled === undefined) {
    // Still loading, or loaded without a usable `enabled` flag. Either way we
    // cannot say whether Code Mode is the reason, so do not assert that it is
    // not.
    return 'No tools returned.'
  }
  return 'No tools exposed by any connected server.'
}

export function ToolBrowser({ initialQuery = '' }: { initialQuery?: string } = {}) {
  const [query, setQuery] = useState(initialQuery)
  const [results, setResults] = useState<ToolSearchHit[]>([])
  const [total, setTotal] = useState(0)
  const [detail, setDetail] = useState<ToolDescription | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<BrowserError | null>(null)
  // The query that produced `results`/`total`, NOT the live input value.
  // `query` changes on every keystroke while results only change on submit,
  // so branching the summary on `query` let a cleared input turn a stale
  // zero-result search into a confident claim about the whole gateway.
  const [executedQuery, setExecutedQuery] = useState<string | null>(null)
  const activeRequest = useRef<AbortController | null>(null)
  const { data: codeModeConfig, error: codeModeConfigError } = useGatewayCodeModeConfig()

  useEffect(() => {
    const clearForSessionChange = () => {
      activeRequest.current?.abort()
      activeRequest.current = null
      setResults([]); setTotal(0); setDetail(null); setError(null); setLoading(false); setExecutedQuery(null)
    }
    const unsubscribe = subscribeToBrowserSession(clearForSessionChange)
    return () => { unsubscribe(); activeRequest.current?.abort() }
  }, [])

  async function runSearch(value: string) {
    activeRequest.current?.abort(); const controller = new AbortController(); activeRequest.current = controller
    setDetail(null); setError(null); setResults([]); setTotal(0)
    setLoading(true)
    try {
      const response = await searchCodeModeTools(value, controller.signal)
      if (activeRequest.current !== controller) return
      setResults(response.results); setTotal(response.total); setExecutedQuery(value)
    } catch (cause) {
      if (activeRequest.current === controller && !isAbortError(cause)) {
        setError({ ...toolBrowserError(cause, 'Tools unavailable'), retry: () => void runSearch(value) })
      }
    } finally { if (activeRequest.current === controller) setLoading(false) }
  }

  async function selectTool(hit: ToolSearchHit) {
    await loadDetail(hit.id)
  }

  async function loadDetail(target: string) {
    activeRequest.current?.abort(); const controller = new AbortController(); activeRequest.current = controller
    setDetail(null); setError(null); setLoading(true)
    try {
      const response = await describeCodeModeTool(target, controller.signal)
      if (activeRequest.current === controller) setDetail(response)
    }
    catch (cause) {
      if (activeRequest.current === controller && !isAbortError(cause)) {
        setError({ ...toolBrowserError(cause, 'Tool not found'), retry: () => void loadDetail(target) })
      }
    }
    finally { if (activeRequest.current === controller) setLoading(false) }
  }

  const summary = resultsSummary({
    total,
    shown: results.length,
    hasError: error !== null,
    executedQuery,
    loading,
    codeModeEnabled: codeModeConfig?.enabled,
    codeModeConfigMissing: codeModeConfig === undefined,
    codeModeConfigFailed: Boolean(codeModeConfigError),
  })

  return <main className="mx-auto w-full max-w-7xl p-6 lg:p-10">
    <div className="mb-8 flex items-start gap-4">
      <div className="rounded-xl border border-aurora-accent-primary/30 bg-aurora-accent-primary/10 p-3"><SearchCode className="size-6 text-aurora-accent-primary" /></div>
      <div><p className="text-xs font-semibold uppercase tracking-[0.2em] text-aurora-accent-primary">Live catalog</p><h1 className="text-3xl font-semibold text-aurora-text-primary">Code Mode tools</h1><p className="mt-2 max-w-2xl text-sm text-aurora-text-secondary">Search the tools visible to this authenticated admin session. Safety facts are advisory; live dispatch remains authoritative.</p></div>
    </div>
    <form className="flex gap-3" onSubmit={(event: FormEvent) => { event.preventDefault(); void runSearch(query) }}>
      <Input aria-label="Search tools" maxLength={1024} value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search by tool, namespace, description, or tag" />
      <Button type="submit" disabled={loading}>{loading ? 'Loading…' : query.trim() ? 'Search' : 'Browse all'}</Button>
    </form>
    {error && <div role="alert" className="mt-4 flex items-center gap-2 rounded-lg border border-aurora-error/40 bg-aurora-error/10 p-3 text-sm"><TriangleAlert className="size-4" /><span>{error.message}{error.requestId ? ` Request ID: ${error.requestId}` : ''}</span>{error.status !== 401 && error.status !== 403 && error.retry && <Button variant="ghost" size="sm" onClick={error.retry}>Retry</Button>}</div>}
    <div className="mt-6 grid gap-6 lg:grid-cols-[minmax(0,1fr)_minmax(22rem,0.8fr)]">
      <section aria-label="Tool results" className="space-y-3">
        <p className="text-xs text-aurora-text-muted">{summary}</p>
        {results.map((hit) => <button key={hit.id} type="button" onClick={() => void selectTool(hit)} className="block w-full rounded-xl border border-aurora-border-default bg-aurora-panel-medium p-4 text-left transition hover:border-aurora-accent-primary/50 hover:bg-aurora-panel-strong">
          <div className="flex items-center justify-between gap-3"><code className="text-sm font-semibold text-aurora-accent-primary">{hit.path}</code><Safety safety={hit.safety} /></div>
          <p className="mt-2 line-clamp-2 text-sm text-aurora-text-secondary">{hit.description || 'No description provided.'}</p>
          <p className="mt-2 truncate font-mono text-xs text-aurora-text-muted">{hit.signature}</p>
        </button>)}
      </section>
      <aside aria-label="Tool details" className="min-h-72 rounded-xl border border-aurora-border-default bg-aurora-panel-medium p-5">
        {!detail ? <div className="flex h-full min-h-64 items-center justify-center text-sm text-aurora-text-muted">Select a tool to inspect its live definition.</div> : <div>
          <div className="flex items-center justify-between gap-3"><code className="text-lg font-semibold text-aurora-accent-primary">{detail.path}</code><Safety safety={detail.safety} /></div>
          <p className="mt-3 text-sm text-aurora-text-secondary">{detail.description}</p>
          <dl className="mt-5 grid gap-2 text-xs"><div><dt className="text-aurora-text-muted">ID</dt><dd className="font-mono">{detail.id}</dd></div><div><dt className="text-aurora-text-muted">Helper</dt><dd className="font-mono">{detail.helper}</dd></div></dl>
          <h2 className="mt-6 text-sm font-semibold">Parameters (TypeScript)</h2>
          {detail.typescript ? <pre className="mt-2 max-h-[32rem] overflow-auto rounded-lg border border-aurora-border-default bg-aurora-bg-primary p-4 text-xs"><code>{detail.typescript}</code></pre> : <p className="mt-2 text-sm text-aurora-text-muted">Parameters unavailable{detail.typescript_omitted === 'size_limit' ? ' because the declaration exceeds the response limit.' : '.'}</p>}
        </div>}
      </aside>
    </div>
  </main>
}

function Safety({ safety }: { safety?: { read_only?: boolean; destructive?: boolean } }) {
  if (!safety) return <span className="text-xs text-aurora-text-muted">Safety unknown</span>
  if (safety.destructive) return <span className="inline-flex items-center gap-1 text-xs text-aurora-warning"><TriangleAlert className="size-3" />Destructive</span>
  if (safety.read_only) return <span className="inline-flex items-center gap-1 text-xs text-aurora-success"><ShieldCheck className="size-3" />Read only</span>
  return <span className="text-xs text-aurora-text-muted">Safety unspecified</span>
}
