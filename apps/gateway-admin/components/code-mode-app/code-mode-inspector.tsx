'use client'

import { type CSSProperties, type ReactNode, useCallback, useEffect, useRef, useState } from 'react'
import {
  AlertTriangle,
  Check,
  ChevronRight,
  Copy,
  CornerDownLeft,
  FileBox,
  History,
  LockKeyhole,
  MoreHorizontal,
  Terminal,
  Wrench,
  X,
} from 'lucide-react'

import { AURORA_BADGE_LABEL } from '@/components/aurora/tokens'
import {
  type CodeModeArtifactReceipt,
  type CodeModeCallUi,
  type CodeModeCallTrace,
  type CodeModeErrorContract,
  type CodeModeExecuteTrace,
  type CodeModeHistoryEntry,
  type CodeModeTrace,
  type DiscoveryResult,
  describeMarkdown,
  describeResultShape,
  parseCodeModeTrace,
  parseDiscoveryResult,
  stringifyRedactedParams,
} from '@/lib/code-mode-app/trace'
import { cn } from '@/lib/utils'

const AURORA_DARK_TOKENS = {
  '--aurora-page-bg': '#07131c',
  '--aurora-panel-strong': '#13293a',
  '--aurora-panel-strong-top': '#173245',
  '--aurora-control-surface': '#0c1a24',
  '--aurora-border-default': '#1d3d4e',
  '--aurora-border-strong': '#24536c',
  '--aurora-text-primary': '#e6f4fb',
  '--aurora-text-muted': '#a7bcc9',
  '--aurora-accent-primary': '#29b6f6',
  '--aurora-accent-strong': '#67cbfa',
  '--aurora-accent-deep': '#1c7fac',
  '--aurora-warn': '#c6a36b',
  '--aurora-error': '#c78490',
  '--aurora-success': '#7dd3c7',
  '--aurora-hover-bg': '#17364b',
  '--aurora-shadow-medium': '0 12px 24px rgba(0, 0, 0, 0.18)',
  '--aurora-highlight-strong': 'inset 0 1px 0 rgba(255, 255, 255, 0.05)',
  '--color-aurora-page-bg': 'var(--aurora-page-bg)',
  '--color-aurora-panel-strong': 'var(--aurora-panel-strong)',
  '--color-aurora-control-surface': 'var(--aurora-control-surface)',
  '--color-aurora-border-default': 'var(--aurora-border-default)',
  '--color-aurora-border-strong': 'var(--aurora-border-strong)',
  '--color-aurora-text-primary': 'var(--aurora-text-primary)',
  '--color-aurora-text-muted': 'var(--aurora-text-muted)',
  '--color-aurora-accent-primary': 'var(--aurora-accent-primary)',
  '--color-aurora-accent-strong': 'var(--aurora-accent-strong)',
  '--color-aurora-warn': 'var(--aurora-warn)',
  '--color-aurora-error': 'var(--aurora-error)',
  '--color-aurora-success': 'var(--aurora-success)',
  '--color-aurora-hover-bg': 'var(--aurora-hover-bg)',
} as CSSProperties

declare global {
  interface Window {
    __LAB_CODE_MODE_INITIAL_TRACE__?: unknown
    // OpenAI Apps runtime (ChatGPT / Codex) injects this; MCP Apps hosts do not.
    openai?: { toolOutput?: unknown; toolInput?: unknown }
    ExtApps?: {
      App?: new (
        appInfo: { name: string; version: string },
        capabilities?: Record<string, unknown>,
        options?: Record<string, unknown>,
      ) => {
        ontoolresult?: (result: { structuredContent?: unknown; structured_content?: unknown }) => void
        ontoolinput?: (params: { arguments?: Record<string, unknown> }) => void
        readServerResource?: ResourceReader
        connect: () => Promise<unknown>
        close?: () => Promise<unknown> | void
      }
    }
  }
}

type ResourceReader = (params: { uri: string }) => Promise<{
  contents?: Array<{
    text?: string
    mimeType?: string
    mime_type?: string
    uri?: string
  }>
}>

/**
 * The LLM's tool-call arguments, delivered by the host (MCP Apps
 * `ui/notifications/tool-input`, or `window.openai.toolInput`). For codemode
 * that is `{ code: "async () => { … }" }` — the snippet that drove the run.
 */
function toolInputSnippet(input: unknown): string | null {
  if (typeof input !== 'object' || input === null || Array.isArray(input)) return null
  const record = input as Record<string, unknown>
  if (typeof record.code === 'string' && record.code.length > 0) return record.code
  if (Object.keys(record).length === 0) return null
  return stringifyRedactedParams(record)
}

interface CodeModeInspectorProps {
  initialTrace?: unknown
}

type RunSelection = 'live' | number

interface InspectorState {
  live: CodeModeExecuteTrace | null
  history: CodeModeHistoryEntry[]
  historyWarnings: string[]
  selected: RunSelection
}

function emptyState(): InspectorState {
  return { live: null, history: [], historyWarnings: [], selected: 'live' }
}

function applyTrace(state: InspectorState, trace: CodeModeTrace): InspectorState {
  if (trace.kind === 'code_mode_execute_trace') {
    return { ...state, live: trace, selected: 'live' }
  }
  const entries = trace.entries
  const selected =
    state.live || entries.length === 0 ? state.selected : entries[entries.length - 1].seq
  return {
    ...state,
    history: entries,
    historyWarnings: trace.warnings?.map((warning) => warning.message) ?? [],
    selected,
  }
}

/**
 * Expansion state for a freshly shown run: the top-level Recovery row opens
 * when an error contract is present, and the first failed call opens so its
 * error_kind and params show without a tap.
 */
function initialExpansion(
  calls: CodeModeCallTrace[] | undefined,
  hasError: boolean,
): Record<string, boolean> {
  const expanded: Record<string, boolean> = hasError ? { error: true } : {}
  const failed = (calls ?? []).findIndex((call) => !call.ok)
  if (failed >= 0) {
    const call = (calls ?? [])[failed]
    expanded[`call:${call.id}-${failed}`] = true
  }
  return expanded
}

export function CodeModeInspector({ initialTrace }: CodeModeInspectorProps) {
  // Parse the initial payload once — initial state, initial expansion, and the
  // accepted-payload identity below all derive from the same parse.
  const [initialParsed] = useState(() => parseCodeModeTrace(initialTrace))
  const [state, setState] = useState<InspectorState>(() =>
    initialParsed ? applyTrace(emptyState(), initialParsed) : emptyState(),
  )
  const [expanded, setExpanded] = useState<Record<string, boolean>>(() =>
    initialParsed?.kind === 'code_mode_execute_trace'
      ? initialExpansion(initialParsed.calls, initialParsed.error !== undefined)
      : {},
  )
  const [toolInput, setToolInput] = useState<unknown>(null)
  const [bridgeWarning, setBridgeWarning] = useState<string | null>(null)
  const [bridgeState, setBridgeState] = useState<'connecting' | 'connected' | 'fallback'>('fallback')
  const [resourceReader, setResourceReader] = useState<ResourceReader | null>(null)
  const [minimized, setMinimized] = useState(false)

  // Serialized identity of the last accepted payload per trace kind. Hosts
  // re-deliver unchanged traces — `openai:set_globals` fires for ANY global
  // change and the sync handler falls back to the live snapshot — and
  // re-accepting one would reset `selected` and wipe `expanded`, collapsing
  // panels the operator has open. The parser output is deterministic, so
  // JSON identity is a sound deep-equality proxy.
  const [initialIdentity] = useState(() => ({
    execute:
      initialParsed?.kind === 'code_mode_execute_trace' ? JSON.stringify(initialParsed) : null,
    history: initialParsed?.kind === 'code_mode_history' ? JSON.stringify(initialParsed) : null,
  }))
  const acceptedRef = useRef({ ...initialIdentity })
  // Execution ids of live runs that a newer live trace has replaced. A late
  // re-delivery of a superseded run (e.g. a stale failed trace arriving after
  // its successful successor) must not repaint a red Recovery banner over the
  // newer result. This guard is deliberately minimal: the wire carries no
  // monotonic sequence number, so a stale run that was never rendered here
  // cannot be detected — full ordering needs a host-side seq. Traces without
  // an execution_id remain last-writer-wins.
  const supersededRef = useRef<Set<string>>(new Set())
  const liveExecutionIdRef = useRef(
    initialParsed?.kind === 'code_mode_execute_trace' ? initialParsed.execution_id : undefined,
  )

  const acceptTrace = useCallback((raw: unknown): boolean => {
    const trace = parseCodeModeTrace(raw)
    if (!trace) return false
    const isExecute = trace.kind === 'code_mode_execute_trace'
    // Parser output is JSON-safe today (all ingress crosses a JSON boundary),
    // but passthrough fields (result, params, evidence) are not re-validated —
    // treat an unserializable trace as new rather than throwing mid-effect,
    // matching stringifyRedactedParams' paranoia.
    let serialized: string | null
    try {
      serialized = JSON.stringify(trace)
    } catch {
      serialized = null
    }
    if (
      serialized !== null &&
      serialized === (isExecute ? acceptedRef.current.execute : acceptedRef.current.history)
    ) {
      // Unchanged re-delivery — keep the operator's expansion and selection.
      setBridgeWarning(null)
      return true
    }
    if (
      isExecute &&
      trace.execution_id !== undefined &&
      supersededRef.current.has(trace.execution_id)
    ) {
      // Stale delivery of a run a newer live trace already replaced.
      setBridgeWarning(null)
      return true
    }
    if (isExecute) {
      const previousId = liveExecutionIdRef.current
      if (previousId !== undefined && previousId !== trace.execution_id) {
        supersededRef.current.add(previousId)
      }
      liveExecutionIdRef.current = trace.execution_id
      acceptedRef.current.execute = serialized
    } else {
      acceptedRef.current.history = serialized
    }
    setState((previous) => applyTrace(previous, trace))
    // A genuinely new trace still gets the fresh auto-open-on-error expansion.
    setExpanded(isExecute ? initialExpansion(trace.calls, trace.error !== undefined) : {})
    setBridgeWarning(null)
    return true
  }, [])

  useEffect(() => {
    const injected = window.__LAB_CODE_MODE_INITIAL_TRACE__
    if (injected !== undefined && !acceptTrace(injected) && injected !== null) {
      setBridgeWarning('Ignored malformed initial trace payload.')
    }

    const App = window.ExtApps?.App
    if (!App) return

    const app = new App(
      { name: 'Lab Code Mode Inspector', version: '0.2.0' },
      {},
      { autoResize: true },
    )
    setResourceReader(() =>
      typeof app.readServerResource === 'function' ? app.readServerResource.bind(app) : null,
    )
    app.ontoolresult = (result) => {
      const payload = result.structuredContent ?? result.structured_content
      if (!acceptTrace(payload)) {
        setBridgeWarning('Ignored malformed bridge payload.')
      }
    }
    // The host streams the tool-call arguments (the snippet the LLM sent)
    // alongside the result — surface them as the run's Input.
    app.ontoolinput = (params) => {
      setToolInput(params?.arguments ?? null)
    }
    setBridgeState('connecting')
    app
      .connect()
      .then(() => setBridgeState('connected'))
      .catch(() => setBridgeState('fallback'))

    return () => {
      setResourceReader(null)
      void app.close?.()
    }
  }, [acceptTrace])

  // OpenAI Apps runtime (ChatGPT / Codex) bridge. These hosts bind the widget
  // via the tool's `openai/outputTemplate` meta and expose the structured tool
  // result on `window.openai.toolOutput` instead of driving the ExtApps
  // `ontoolresult` path, so hydrate from it directly and track live updates.
  useEffect(() => {
    if (!window.openai) return
    // The openai:set_globals CustomEvent carries changed values on
    // event.detail.globals; prefer that, falling back to the live snapshot.
    const sync = (event?: Event) => {
      const globals = (event as CustomEvent<{ globals?: Record<string, unknown> }> | undefined)?.detail
        ?.globals
      // The event's globals are authoritative for the changed key (including an
      // explicit null clear); only without it do we read the live snapshot.
      const hasInputKey =
        globals != null && Object.prototype.hasOwnProperty.call(globals, 'toolInput')
      const rawInput = hasInputKey ? globals.toolInput : window.openai?.toolInput
      if (rawInput !== undefined) setToolInput(rawInput)
      const hasKey = globals != null && Object.prototype.hasOwnProperty.call(globals, 'toolOutput')
      const raw = hasKey ? globals.toolOutput : window.openai?.toolOutput
      if (acceptTrace(raw)) {
        setBridgeState('connected')
      } else if (raw != null) {
        // Present but unparseable — surface it like the ExtApps path does
        // instead of silently dropping the host's payload.
        setBridgeWarning('Ignored malformed bridge payload.')
      } else if (hasKey) {
        // Host explicitly cleared the result — drop the stale trace and its
        // accepted identity so a later re-delivery of the same run renders.
        // An explicit clear starts a new epoch: ordering knowledge from the
        // old epoch is itself stale, so the superseded set resets too — a
        // cleared-then-resent run must render, not stay blank.
        acceptedRef.current.execute = null
        acceptedRef.current.history = null
        liveExecutionIdRef.current = undefined
        supersededRef.current.clear()
        setState(emptyState())
        setExpanded({})
        setBridgeWarning(null)
      }
    }
    sync()
    window.addEventListener('openai:set_globals', sync)
    return () => window.removeEventListener('openai:set_globals', sync)
  }, [acceptTrace])

  const toggle = (key: string) => {
    setExpanded((previous) => ({ ...previous, [key]: !previous[key] }))
  }

  const selectedEntry =
    state.selected === 'live' ? null : state.history.find((entry) => entry.seq === state.selected)
  const live = state.selected === 'live' ? state.live : null
  const run = live ?? selectedEntry ?? null
  const calls: CodeModeCallTrace[] = live ? live.calls : (selectedEntry?.calls ?? [])
  const runOk = live
    ? live.error_kind === undefined && calls.every((call) => call.ok)
    : (selectedEntry?.ok ?? true)
  const errorKind = live
    ? (live.error_kind ?? calls.find((call) => !call.ok)?.error_kind)
    : selectedEntry?.error_kind
  const elapsedMs = live
    ? (live.elapsed_ms ??
      state.history.find(
        (entry) => entry.execution_id !== undefined && entry.execution_id === live.execution_id,
      )?.elapsed_ms)
    : selectedEntry?.elapsed_ms
  const tokens = live ?? selectedEntry
  const discovery = live ? parseDiscoveryResult(live.result) : null
  const describeDoc = live ? describeMarkdown(live.result) : null
  // Host-delivered tool-call arguments apply to the live run only.
  const inputSnippet = live ? toolInputSnippet(toolInput) : null
  const activeUi = [...calls].reverse().find((call) => call.ui)?.ui ?? null
  const activeUiResourceUri = activeUi?.resourceUri ?? null
  const warnings = [
    ...(bridgeWarning ? [bridgeWarning] : []),
    ...(state.live?.warnings?.map((warning) => warning.message) ?? []),
    ...state.historyWarnings,
  ]

  useEffect(() => {
    setMinimized(Boolean(activeUiResourceUri))
  }, [activeUiResourceUri])

  return (
    <main
      className="min-h-[100dvh] bg-aurora-page-bg p-4 font-sans text-aurora-text-primary"
      style={{
        ...AURORA_DARK_TOKENS,
        background:
          'radial-gradient(900px 420px at 12% -10%, rgba(41,182,246,0.08), transparent 60%), var(--aurora-page-bg)',
      }}
    >
      <div className="mx-auto w-full max-w-[680px]">
      <section
        className={cn('w-full overflow-hidden border', minimized ? 'rounded-[10px]' : 'rounded-[18px]')}
        style={{
          background: 'linear-gradient(180deg, var(--aurora-panel-strong-top), var(--aurora-panel-strong))',
          borderColor: 'color-mix(in srgb, var(--aurora-border-default) 45%, var(--aurora-page-bg))',
          boxShadow: 'var(--aurora-shadow-medium), var(--aurora-highlight-strong)',
        }}
      >
        <WidgetHead
          calls={run ? calls.length : null}
          matches={discovery?.hits.length}
          describe={describeDoc !== null}
          ok={runOk}
          errorKind={errorKind}
          elapsedMs={elapsedMs}
          inputTokens={tokens?.input_tokens}
          outputTokens={tokens?.output_tokens}
          logsCount={live?.logs_count}
          // A rendered trace proves the bridge works — the state label only
          // earns its place while the card is empty, explaining why.
          bridgeLabel={run ? null : bridgeState}
          minimized={minimized}
          onToggleMinimized={() => setMinimized((current) => !current)}
          history={state.history}
          live={state.live}
          selected={state.selected}
          onSelect={(selection) => {
            setState((previous) => ({ ...previous, selected: selection }))
            const entry =
              selection === 'live'
                ? null
                : state.history.find((candidate) => candidate.seq === selection)
            setExpanded(
              selection === 'live'
                ? initialExpansion(state.live?.calls, state.live?.error !== undefined)
                : // History entries never retain an error contract — calls only.
                  initialExpansion(entry?.calls, false),
            )
          }}
        />

        {!minimized ? (
          <>
            {warnings.map((warning, index) => (
              <WarnLine key={`${warning}-${index}`} message={warning} />
            ))}

            {!run ? (
              <p className="px-3 py-4 text-center text-xs text-aurora-text-muted">
                Waiting for an MCP Apps tool result or history snapshot.
              </p>
            ) : (
              // Cap the rows region (~10 rows) so long runs scroll internally
              // instead of growing the inline card unbounded — the MCP host sizes
              // the iframe to the document height.
              <div className="aurora-scrollbar max-h-[300px] overflow-y-auto overscroll-contain">
                {live?.error ? (
                  <ErrorRow
                    error={live.error}
                    open={Boolean(expanded.error)}
                    onToggle={() => toggle('error')}
                  />
                ) : null}
                {calls.length > 0 ? (
                  <CallRows calls={calls} expanded={expanded} onToggle={toggle} />
                ) : live && live.result !== undefined ? null : (
                  <p className="px-3 py-3 text-xs text-aurora-text-muted">No calls were made.</p>
                )}
                {discovery ? (
                  <DiscoveryRows discovery={discovery} expanded={expanded} onToggle={toggle} />
                ) : null}
                {inputSnippet ? (
                  <InputRow
                    snippet={inputSnippet}
                    open={Boolean(expanded.input)}
                    onToggle={() => toggle('input')}
                  />
                ) : null}
                {live && live.result !== undefined ? (
                  <ResultRow
                    trace={live}
                    markdown={describeDoc}
                    open={Boolean(expanded.result)}
                    onToggle={() => toggle('result')}
                  />
                ) : null}
                {live?.artifacts?.length ? (
                  <ArtifactsRow
                    artifacts={live.artifacts}
                    open={Boolean(expanded.artifacts)}
                    onToggle={() => toggle('artifacts')}
                  />
                ) : null}
                {selectedEntry ? <HistoryNote /> : null}
              </div>
            )}

          </>
        ) : null}
      </section>
      {activeUi ? <McpUiResourcePanel ui={activeUi} resourceReader={resourceReader} /> : null}
      </div>
    </main>
  )
}

function formatMs(ms: number): string {
  return ms >= 1000 ? `${(ms / 1000).toFixed(2).replace(/0$/, '')} s` : `${ms} ms`
}

const HAIRLINE = 'color-mix(in srgb, var(--aurora-border-default) 30%, transparent)'
const HEAD_FOOT_BG = 'color-mix(in srgb, var(--aurora-page-bg) 25%, transparent)'
const HEAD_FOOT_BORDER = 'color-mix(in srgb, var(--aurora-border-default) 50%, var(--aurora-page-bg))'

function WidgetHead({
  calls,
  matches,
  describe,
  ok,
  errorKind,
  elapsedMs,
  inputTokens,
  outputTokens,
  logsCount,
  bridgeLabel,
  minimized,
  onToggleMinimized,
  history,
  live,
  selected,
  onSelect,
}: {
  calls: number | null
  matches: number | undefined
  describe: boolean
  ok: boolean
  errorKind: string | undefined
  elapsedMs: number | undefined
  inputTokens: number | undefined
  outputTokens: number | undefined
  logsCount: number | undefined
  bridgeLabel: string | null
  minimized: boolean
  onToggleMinimized: () => void
  history: CodeModeHistoryEntry[]
  live: CodeModeExecuteTrace | null
  selected: RunSelection
  onSelect: (selection: RunSelection) => void
}) {
  const meta =
    calls === null
      ? []
      : [
          `${calls} call${calls === 1 ? '' : 's'}`,
          elapsedMs !== undefined ? formatMs(elapsedMs) : null,
          inputTokens !== undefined || outputTokens !== undefined
            ? `${inputTokens ?? 0}→${outputTokens ?? 0} tokens`
            : null,
          matches !== undefined ? `${matches} match${matches === 1 ? '' : 'es'}` : null,
          describe ? 'describe' : null,
          logsCount ? `${logsCount} log${logsCount === 1 ? '' : 's'}` : null,
        ].filter((item): item is string => item !== null)
  return (
    <div
      className={cn('flex min-w-0 items-center gap-2 px-3', minimized ? 'py-1.5' : 'border-b py-2')}
      style={{ borderColor: minimized ? 'transparent' : HEAD_FOOT_BORDER, background: HEAD_FOOT_BG }}
    >
      <LabbyMark />
      <span className="text-[12.5px] font-bold">Execute</span>
      {calls !== null ? (
        ok ? (
          <StatusDot tone="success" label="success" />
        ) : (
          <span className={cn(AURORA_BADGE_LABEL, 'text-aurora-error')}>{errorKind ?? 'error'}</span>
        )
      ) : null}
      {meta.length > 0 ? (
        <span className="min-w-0 truncate text-[11px] font-medium tabular-nums text-aurora-text-muted">
          {meta.join(' · ')}
        </span>
      ) : null}
      <span className="flex-1" />
      {bridgeLabel !== null && bridgeLabel !== 'connected' ? (
        <span className={cn(AURORA_BADGE_LABEL, 'text-aurora-text-muted')}>{bridgeLabel}</span>
      ) : null}
      <RunMenu history={history} live={live} selected={selected} onSelect={onSelect} />
      <LockKeyhole
        aria-label="Read only"
        className="size-3 shrink-0 text-aurora-text-muted"
        strokeWidth={1.75}
      />
      <button
        type="button"
        aria-label={minimized ? 'Restore inspector' : 'Minimize inspector'}
        aria-pressed={minimized}
        title={minimized ? 'Restore inspector' : 'Minimize inspector'}
        onClick={onToggleMinimized}
        className="flex size-5 shrink-0 items-center justify-center rounded border border-transparent text-aurora-text-muted transition-colors hover:border-aurora-border-strong hover:text-aurora-text-primary"
      >
        <ChevronRight
          className={cn('size-3 transition-transform', minimized ? 'rotate-90' : '-rotate-90')}
          strokeWidth={1.75}
        />
      </button>
    </div>
  )
}

function RunMenu({
  history,
  live,
  selected,
  onSelect,
}: {
  history: CodeModeHistoryEntry[]
  live: CodeModeExecuteTrace | null
  selected: RunSelection
  onSelect: (selection: RunSelection) => void
}) {
  const liveSeq =
    live?.execution_id !== undefined
      ? history.find((entry) => entry.execution_id === live.execution_id)?.seq
      : undefined
  const runs: { key: string; label: string; ok: boolean; target: RunSelection }[] = history.map(
    (entry) => ({
      key: `seq-${entry.seq}`,
      label: entry.seq === liveSeq ? `Run #${entry.seq} · live` : `Run #${entry.seq}`,
      ok: entry.ok,
      target: entry.seq === liveSeq ? 'live' : entry.seq,
    }),
  )
  if (live && liveSeq === undefined) {
    runs.push({
      key: 'live',
      label: 'Live',
      ok: live.calls.every((call) => call.ok),
      target: 'live',
    })
  }
  if (runs.length < 2) return null
  return (
    <details className="group relative">
      <summary
        aria-label="Run history"
        title="Run history"
        className="flex size-5 cursor-pointer list-none items-center justify-center rounded border border-transparent text-aurora-text-muted transition-colors hover:border-aurora-border-strong hover:text-aurora-text-primary"
      >
        <MoreHorizontal className="size-3" strokeWidth={1.75} />
      </summary>
      <div
        className="absolute right-0 top-6 z-20 grid min-w-36 gap-1 rounded-lg border p-1.5"
        style={{
          borderColor: 'var(--aurora-border-strong)',
          background: 'var(--aurora-control-surface)',
          boxShadow: 'var(--aurora-shadow-medium)',
        }}
      >
        <span className={cn(AURORA_BADGE_LABEL, 'flex items-center gap-1.5 px-1.5 py-1 text-aurora-text-muted')}>
          <History className="size-3" strokeWidth={1.75} />
          Run history
        </span>
        {runs.map((run) => {
          const active = run.target === selected
          return (
            <button
              key={run.key}
              type="button"
              onClick={(event) => {
                onSelect(run.target)
                event.currentTarget.closest('details')?.removeAttribute('open')
              }}
              className={cn(
                'flex items-center gap-2 rounded px-2 py-1 text-left text-[11px] text-aurora-text-muted hover:bg-aurora-hover-bg/60 hover:text-aurora-text-primary',
                active && 'bg-aurora-hover-bg/60 text-aurora-text-primary',
              )}
            >
              <StatusDot tone={run.ok ? 'success' : 'error'} label={run.ok ? 'success' : 'failed'} />
              {run.label}
            </button>
          )
        })}
      </div>
    </details>
  )
}

function LabbyMark() {
  return (
    <svg
      aria-hidden="true"
      className="size-[15px] shrink-0 text-aurora-accent-strong"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.65"
      strokeLinecap="round"
    >
      <path d="M12 3v18M3 12h18M6.7 6.7l10.6 10.6M17.3 6.7 6.7 17.3" />
    </svg>
  )
}

function StatusDot({ tone, label }: { tone: 'success' | 'error'; label: string }) {
  return (
    <span
      aria-label={label}
      role="img"
      className={cn(
        'inline-block size-[5px] shrink-0 rounded-full',
        tone === 'success' ? 'bg-aurora-success' : 'bg-aurora-error',
      )}
      style={{ boxShadow: '0 0 4px currentColor', color: tone === 'success' ? 'var(--aurora-success)' : 'var(--aurora-error)' }}
    />
  )
}

function WarnLine({ message }: { message: string }) {
  return (
    <p
      className="flex items-center gap-2 border-b px-3 py-1.5 text-[11px] text-aurora-warn"
      style={{
        borderColor: 'color-mix(in srgb, var(--aurora-warn) 22%, transparent)',
        background: 'color-mix(in srgb, var(--aurora-warn) 8%, transparent)',
      }}
    >
      <AlertTriangle className="size-3 shrink-0" strokeWidth={1.75} />
      {message}
    </p>
  )
}

function humanizeErrorToken(value: string): string {
  return value.replaceAll('_', ' ')
}

function ErrorRow({
  error,
  open,
  onToggle,
}: {
  error: CodeModeErrorContract
  open: boolean
  onToggle: () => void
}) {
  // Stringification only matters once the panel is open — skip it while the
  // row is collapsed.
  const evidence = open && error.evidence ? stringifyRedactedParams(error.evidence) : ''
  const safety = open && error.safety ? stringifyRedactedParams(error.safety) : ''
  // Raw tokens with slot-prefixed keys (kind/origin can carry the same value);
  // humanization happens exactly once, in render.
  const badges: { key: string; token: string }[] = [
    { key: `k:${error.kind}`, token: error.kind },
    { key: `o:${error.origin}`, token: error.origin },
    { key: `s:${error.recovery.same_arguments}`, token: `same args: ${error.recovery.same_arguments}` },
  ]
  if (error.recovery.retry_after_ms !== undefined) {
    badges.push({ key: 'r:retry_after', token: `retry after ${formatMs(error.recovery.retry_after_ms)}` })
  }
  return (
    <div>
      <button
        type="button"
        onClick={onToggle}
        className="grid w-full cursor-pointer grid-cols-[14px_minmax(0,auto)_minmax(30px,1fr)_13px] items-center gap-2 border-t px-3 py-1.5 text-left transition-colors first:border-t-0 hover:bg-aurora-hover-bg/40"
        style={{ borderColor: HAIRLINE }}
      >
        <AlertTriangle className="size-3 text-aurora-error" strokeWidth={1.75} />
        <span className={cn(AURORA_BADGE_LABEL, 'text-aurora-error')}>Recovery</span>
        <span className="truncate text-[11px] text-aurora-text-muted">
          {error.tool ? `${error.tool} · ` : ''}
          {humanizeErrorToken(error.recovery.action)} · side effects {humanizeErrorToken(error.side_effects)}
        </span>
        <ChevronRight
          className={cn('size-3 text-aurora-text-muted transition-transform', open && 'rotate-90')}
          strokeWidth={1.75}
        />
      </button>
      {open ? (
        <div className="flex flex-col gap-2 px-3 pb-3 pl-[34px]">
          <p className="text-xs leading-relaxed text-aurora-text-primary">{error.message}</p>
          <div className="flex flex-wrap gap-1.5">
            {badges.map((badge) => (
              <span
                key={badge.key}
                className="rounded-full border px-2 py-0.5 text-[10px] font-semibold text-aurora-text-muted"
                style={{ borderColor: HAIRLINE }}
              >
                {humanizeErrorToken(badge.token)}
              </span>
            ))}
          </div>
          <div>
            <span className={cn(AURORA_BADGE_LABEL, 'text-aurora-text-muted')}>Next Action</span>
            <p className="mt-1 text-[11px] leading-relaxed text-aurora-text-muted">
              {error.recovery.guidance}
            </p>
          </div>
          {error.cause ? (
            <div>
              <span className={cn(AURORA_BADGE_LABEL, 'text-aurora-text-muted')}>Cause</span>
              <div className="mt-1">
                <CodeBlock value={error.cause} />
              </div>
            </div>
          ) : null}
          {safety ? (
            <div>
              <span className={cn(AURORA_BADGE_LABEL, 'text-aurora-text-muted')}>Safety Hints</span>
              <div className="mt-1">
                <CodeBlock value={safety} />
              </div>
            </div>
          ) : null}
          {evidence ? (
            <div>
              <span className={cn(AURORA_BADGE_LABEL, 'text-aurora-text-muted')}>Evidence</span>
              <div className="mt-1">
                <CodeBlock value={evidence} />
              </div>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  )
}

function CallRows({
  calls,
  expanded,
  onToggle,
}: {
  calls: CodeModeCallTrace[]
  expanded: Record<string, boolean>
  onToggle: (key: string) => void
}) {
  const maxElapsed = Math.max(...calls.map((call) => call.elapsed_ms), 1)
  // When start offsets are present (newer traces), bars form a true waterfall
  // over the run span; otherwise they fall back to relative duration bars.
  const hasOffsets = calls.some((call) => call.start_ms !== undefined)
  const span = hasOffsets
    ? Math.max(...calls.map((call) => (call.start_ms ?? 0) + call.elapsed_ms), 1)
    : maxElapsed
  return (
    <div>
      {calls.map((call, index) => {
        const key = `call:${call.id}-${index}`
        const open = Boolean(expanded[key])
        const params = stringifyRedactedParams(call.params)
        const left = hasOffsets ? ((call.start_ms ?? 0) / span) * 100 : 0
        const width = Math.min(
          Math.max((call.elapsed_ms / span) * 100, 4),
          100 - left,
        )
        return (
          <div key={key}>
            <button
              type="button"
              onClick={() => onToggle(key)}
              className="grid w-full cursor-pointer grid-cols-[14px_minmax(0,auto)_minmax(30px,1fr)_52px_13px] items-center gap-2 border-t px-3 py-1.5 text-left transition-colors first:border-t-0 hover:bg-aurora-hover-bg/40"
              style={{ borderColor: index === 0 ? 'transparent' : HAIRLINE }}
            >
              <span className="flex items-center justify-center">
                {call.ok ? (
                  <Check
                    aria-label="success"
                    role="img"
                    className="size-3 shrink-0 text-aurora-success"
                    strokeWidth={2}
                  />
                ) : (
                  <X
                    aria-label="failed"
                    role="img"
                    className="size-3 shrink-0 text-aurora-error"
                    strokeWidth={2}
                  />
                )}
              </span>
              <span className="flex min-w-0 items-baseline gap-1.5">
                <span className="shrink-0 text-[10px] font-semibold uppercase tracking-[0.04em] text-aurora-text-muted">
                  {call.upstream}
                </span>
                <span className="truncate text-xs font-semibold">{call.tool}</span>
              </span>
              <span
                className="relative h-1 rounded-full"
                style={{ background: 'color-mix(in srgb, var(--aurora-border-default) 34%, transparent)' }}
              >
                <span
                  className="absolute inset-y-0 min-w-1 rounded-full"
                  style={{
                    left: `${left.toFixed(1)}%`,
                    width: `${width.toFixed(1)}%`,
                    background: call.ok
                      ? 'linear-gradient(90deg, var(--aurora-accent-deep), var(--aurora-accent-primary))'
                      : 'linear-gradient(90deg, color-mix(in srgb, var(--aurora-error) 70%, var(--aurora-page-bg)), var(--aurora-error))',
                  }}
                />
              </span>
              <span className="text-right text-[11px] font-semibold tabular-nums text-aurora-text-muted">
                {formatMs(call.elapsed_ms)}
              </span>
              <ChevronRight
                className={cn('size-3 text-aurora-text-muted transition-transform', open && 'rotate-90')}
                strokeWidth={1.75}
              />
            </button>
            {open ? (
              <div className="flex flex-col gap-1.5 px-3 pb-2 pl-[34px]">
                {!call.ok && call.error_kind ? (
                  <p className="text-[11px] text-aurora-error">{call.error_kind}</p>
                ) : null}
                {params ? (
                  <>
                    <span className={cn(AURORA_BADGE_LABEL, 'text-aurora-text-muted')}>
                      Redacted Params
                    </span>
                    <CodeBlock value={params} />
                  </>
                ) : null}
              </div>
            ) : null}
          </div>
        )
      })}
    </div>
  )
}

function McpUiResourcePanel({
  ui,
  resourceReader,
}: {
  ui: CodeModeCallUi
  resourceReader: ResourceReader | null
}) {
  const [html, setHtml] = useState<string | null>(null)
  const [state, setState] = useState<'idle' | 'loading' | 'ready' | 'unavailable' | 'error'>(
    resourceReader ? 'loading' : 'unavailable',
  )

  useEffect(() => {
    if (!resourceReader) {
      setHtml(null)
      setState('unavailable')
      return
    }
    let cancelled = false
    setState('loading')
    setHtml(null)
    resourceReader({ uri: ui.resourceUri })
      .then((result) => {
        if (cancelled) return
        const content = result.contents?.find((item) => {
          const mime = item.mimeType ?? item.mime_type ?? ''
          return typeof item.text === 'string' && (mime.includes('html') || item.uri === ui.resourceUri)
        })
        if (content?.text) {
          setHtml(content.text)
          setState('ready')
        } else {
          setState('unavailable')
        }
      })
      .catch(() => {
        if (!cancelled) setState('error')
      })
    return () => {
      cancelled = true
    }
  }, [resourceReader, ui.resourceUri])

  return (
    <section
      className="mt-2 overflow-hidden rounded-[10px] border"
      style={{
        borderColor: 'color-mix(in srgb, var(--aurora-border-default) 45%, var(--aurora-page-bg))',
        background: 'linear-gradient(180deg, var(--aurora-panel-strong-top), var(--aurora-panel-strong))',
        boxShadow: 'var(--aurora-shadow-medium), var(--aurora-highlight-strong)',
      }}
    >
      <div
        className="flex min-w-0 items-center gap-2 border-b px-3 py-2"
        style={{
        borderColor: HAIRLINE,
        background: HEAD_FOOT_BG,
      }}
      >
        <span
          className="flex size-7 shrink-0 items-center justify-center rounded-md border text-aurora-accent-strong"
          style={{
            borderColor: 'color-mix(in srgb, var(--aurora-accent-primary) 42%, var(--aurora-border-default))',
            background: 'color-mix(in srgb, var(--aurora-accent-primary) 9%, var(--aurora-control-surface))',
          }}
        >
          <Terminal className="size-3.5" strokeWidth={1.75} />
        </span>
        <span className="shrink-0 text-[12.5px] font-bold">MCP App</span>
        <span className="min-w-0 truncate text-[11.5px] text-aurora-text-muted" title={ui.resourceUri}>
          {externalAppName(ui.resourceUri)}
        </span>
        {state === 'loading' ? (
          <span className={cn(AURORA_BADGE_LABEL, 'ml-auto shrink-0 text-aurora-text-muted')}>
            loading
          </span>
        ) : state === 'ready' ? (
          <span className="ml-auto flex shrink-0 items-center gap-1.5 text-[11px] text-aurora-success">
            <StatusDot tone="success" label="ready" />
            Ready
          </span>
        ) : null}
        {state === 'error' ? (
          <span className={cn(AURORA_BADGE_LABEL, 'ml-auto shrink-0 text-aurora-warn')}>
            unavailable
          </span>
        ) : null}
      </div>
      <div className="min-h-[220px] bg-white">
        {html ? (
          <iframe
            title={`${ui.resourceUri} MCP UI`}
            className="block min-h-[320px] w-full border-0 bg-white"
            style={{ height: 'min(620px, 72vh)' }}
            sandbox="allow-scripts allow-forms allow-popups allow-downloads"
            srcDoc={html}
          />
        ) : (
          <div className="flex min-h-[220px] items-center justify-center px-5 text-center text-xs text-[#4a6872]">
            {state === 'loading'
              ? 'Loading MCP UI...'
              : state === 'error'
                ? 'Failed to load MCP UI resource.'
                : 'MCP UI resource reader unavailable.'}
          </div>
        )}
      </div>
    </section>
  )
}

function externalAppName(resourceUri: string): string {
  try {
    return new URL(resourceUri).hostname || resourceUri
  } catch {
    return resourceUri
  }
}

function DiscoveryRows({
  discovery,
  expanded,
  onToggle,
}: {
  discovery: DiscoveryResult
  expanded: Record<string, boolean>
  onToggle: (key: string) => void
}) {
  if (discovery.hits.length === 0) {
    return (
      <p className="px-3 py-3 text-xs text-aurora-text-muted">
        {discovery.hint ?? 'No matches.'}
      </p>
    )
  }
  return (
    <div>
      {discovery.hits.map((hit, index) => {
        const key = `hit:${hit.id}-${index}`
        const open = Boolean(expanded[key])
        const meta = [
          hit.path,
          hit.kind,
          hit.score !== undefined ? `score ${hit.score.toFixed(2)}` : undefined,
        ]
          .filter(Boolean)
          .join(' · ')
        return (
          <div key={key}>
            <button
              type="button"
              onClick={() => onToggle(key)}
              className="grid w-full cursor-pointer grid-cols-[14px_minmax(0,1fr)_13px] items-center gap-2 border-t px-3 py-1.5 text-left transition-colors first:border-t-0 hover:bg-aurora-hover-bg/40"
              style={{ borderColor: index === 0 ? 'transparent' : HAIRLINE }}
            >
              <Wrench className="size-3 text-aurora-accent-primary" strokeWidth={1.75} />
              <span className="flex min-w-0 items-baseline gap-1.5">
                {hit.namespace ? (
                  <span className="shrink-0 text-[10px] font-semibold uppercase tracking-[0.04em] text-aurora-text-muted">
                    {hit.namespace}
                  </span>
                ) : null}
                <span className="shrink-0 text-xs font-semibold">{hit.name ?? hit.id}</span>
                {hit.description ? (
                  <span className="truncate text-[11px] text-aurora-text-muted">{hit.description}</span>
                ) : null}
              </span>
              <ChevronRight
                className={cn('size-3 text-aurora-text-muted transition-transform', open && 'rotate-90')}
                strokeWidth={1.75}
              />
            </button>
            {open ? (
              <div className="flex flex-col gap-1.5 px-3 pb-2 pl-[34px]">
                {hit.description ? (
                  <p className="text-[11px] leading-relaxed text-aurora-text-muted">{hit.description}</p>
                ) : null}
                {meta ? <p className="text-[10.5px] text-aurora-text-muted">{meta}</p> : null}
                {hit.signature ? <CodeBlock value={hit.signature} /> : null}
              </div>
            ) : null}
          </div>
        )
      })}
    </div>
  )
}

function InputRow({
  snippet,
  open,
  onToggle,
}: {
  snippet: string
  open: boolean
  onToggle: () => void
}) {
  const lines = snippet.split('\n').length
  return (
    <div>
      <button
        type="button"
        onClick={onToggle}
        className="grid w-full cursor-pointer grid-cols-[14px_minmax(0,auto)_minmax(30px,1fr)_13px] items-center gap-2 border-t px-3 py-1.5 text-left transition-colors hover:bg-aurora-hover-bg/40"
        style={{ borderColor: HAIRLINE }}
      >
        <Terminal className="size-3 text-aurora-accent-primary" strokeWidth={1.75} />
        <span className={cn(AURORA_BADGE_LABEL, 'text-aurora-text-muted')}>Input</span>
        <span className="truncate text-[11px] text-aurora-text-muted">
          {lines} line{lines === 1 ? '' : 's'}
        </span>
        <ChevronRight
          className={cn('size-3 text-aurora-text-muted transition-transform', open && 'rotate-90')}
          strokeWidth={1.75}
        />
      </button>
      {open ? (
        <div className="px-3 pb-2 pl-[34px]">
          <CodeBlock value={snippet} />
        </div>
      ) : null}
    </div>
  )
}

function ResultRow({
  trace,
  markdown,
  open,
  onToggle,
}: {
  trace: CodeModeExecuteTrace
  markdown: string | null
  open: boolean
  onToggle: () => void
}) {
  const shape = describeResultShape(trace.result_shape)
  return (
    <div>
      <button
        type="button"
        onClick={onToggle}
        className="grid w-full cursor-pointer grid-cols-[14px_minmax(0,auto)_minmax(30px,1fr)_13px] items-center gap-2 border-t px-3 py-1.5 text-left transition-colors hover:bg-aurora-hover-bg/40"
        style={{ borderColor: HAIRLINE }}
      >
        <CornerDownLeft className="size-3 text-aurora-accent-primary" strokeWidth={1.75} />
        <span className={cn(AURORA_BADGE_LABEL, 'text-aurora-text-muted')}>Result</span>
        <span className="flex min-w-0 items-center gap-1.5">
          <span className="truncate text-[11px] text-aurora-text-muted">{shape}</span>
          {trace.result_shape?.truncated ? (
            <span className={cn(AURORA_BADGE_LABEL, 'shrink-0 text-aurora-warn')}>truncated</span>
          ) : null}
        </span>
        <ChevronRight
          className={cn('size-3 text-aurora-text-muted transition-transform', open && 'rotate-90')}
          strokeWidth={1.75}
        />
      </button>
      {open ? (
        <div className="px-3 pb-2 pl-[34px]">
          {markdown !== null ? (
            <MarkdownDoc source={markdown} />
          ) : (
            <CodeBlock value={stringifyRedactedParams(trace.result)} />
          )}
        </div>
      ) : null}
    </div>
  )
}

function ArtifactsRow({
  artifacts,
  open,
  onToggle,
}: {
  artifacts: CodeModeArtifactReceipt[]
  open: boolean
  onToggle: () => void
}) {
  return (
    <div>
      <button
        type="button"
        onClick={onToggle}
        className="grid w-full cursor-pointer grid-cols-[14px_minmax(0,auto)_minmax(30px,1fr)_13px] items-center gap-2 border-t px-3 py-1.5 text-left transition-colors hover:bg-aurora-hover-bg/40"
        style={{ borderColor: HAIRLINE }}
      >
        <FileBox className="size-3 text-aurora-accent-primary" strokeWidth={1.75} />
        <span className={cn(AURORA_BADGE_LABEL, 'text-aurora-text-muted')}>Artifacts</span>
        <span className="truncate text-[11px] text-aurora-text-muted">
          {artifacts.length} file{artifacts.length === 1 ? '' : 's'}
        </span>
        <ChevronRight
          className={cn('size-3 text-aurora-text-muted transition-transform', open && 'rotate-90')}
          strokeWidth={1.75}
        />
      </button>
      {open ? (
        <div className="flex flex-col gap-1 px-3 pb-2 pl-[34px]">
          {artifacts.map((artifact, index) => (
            <p key={`${artifact.path}-${index}`} className="flex min-w-0 items-baseline gap-1.5">
              <span className="truncate text-xs font-semibold">{artifact.path}</span>
              <span className="shrink-0 text-[10.5px] text-aurora-text-muted">
                {[artifact.content_type, artifact.bytes !== undefined ? `${artifact.bytes} B` : undefined]
                  .filter(Boolean)
                  .join(' · ')}
              </span>
            </p>
          ))}
        </div>
      ) : null}
    </div>
  )
}

/**
 * Minimal renderer for the markdown subset `codemode.describe()` emits:
 * headings, fenced code, bullet lists, inline code, paragraphs.
 */
function MarkdownDoc({ source }: { source: string }) {
  const blocks: ReactNode[] = []
  const lines = source.split('\n')
  let index = 0
  let key = 0
  while (index < lines.length) {
    const line = lines[index]
    if (line.startsWith('```')) {
      const fence: string[] = []
      index += 1
      while (index < lines.length && !lines[index].startsWith('```')) {
        fence.push(lines[index])
        index += 1
      }
      index += 1
      blocks.push(<CodeBlock key={key++} value={fence.join('\n')} />)
      continue
    }
    const heading = /^(#{1,3})\s+(.*)$/.exec(line)
    if (heading) {
      blocks.push(
        <p key={key++} className="text-xs font-bold text-aurora-text-primary">
          {renderInline(heading[2])}
        </p>,
      )
      index += 1
      continue
    }
    if (line.startsWith('- ')) {
      const items: string[] = []
      while (index < lines.length && lines[index].startsWith('- ')) {
        items.push(lines[index].slice(2))
        index += 1
      }
      blocks.push(
        <ul key={key++} className="list-disc pl-4 text-[11px] leading-relaxed text-aurora-text-muted">
          {items.map((item, itemIndex) => (
            <li key={itemIndex}>{renderInline(item)}</li>
          ))}
        </ul>,
      )
      continue
    }
    if (line.trim().length > 0) {
      blocks.push(
        <p key={key++} className="text-[11px] leading-relaxed text-aurora-text-muted">
          {renderInline(line)}
        </p>,
      )
    }
    index += 1
  }
  return <div className="flex flex-col gap-1.5">{blocks}</div>
}

function renderInline(text: string): ReactNode[] {
  // Split on `code` spans; everything else renders as plain text.
  return text.split(/(`[^`]+`)/).map((segment, index) =>
    segment.startsWith('`') && segment.endsWith('`') && segment.length > 1 ? (
      <code
        key={index}
        className="rounded px-1 font-mono text-[10.5px] text-aurora-text-primary"
        style={{ background: 'color-mix(in srgb, var(--aurora-page-bg) 55%, var(--aurora-control-surface))' }}
      >
        {segment.slice(1, -1)}
      </code>
    ) : (
      segment
    ),
  )
}

function HistoryNote() {
  return (
    <p
      className="flex items-center gap-2 border-t px-3 py-1.5 text-[11px] text-aurora-text-muted"
      style={{ borderColor: HAIRLINE }}
    >
      <History className="size-3 shrink-0" strokeWidth={1.75} />
      Result not retained in history — params and call outcomes only.
    </p>
  )
}

function CodeBlock({ value }: { value: string }) {
  const [copied, setCopied] = useState(false)
  const copiedTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  // Cancelled-flag pattern (see McpUiResourcePanel): the clipboard promise can
  // resolve after unmount, and its .then must neither set state nor schedule
  // the feedback timer once cleanup has already run.
  const cancelledRef = useRef(false)
  useEffect(() => {
    cancelledRef.current = false
    return () => {
      cancelledRef.current = true
      if (copiedTimer.current !== null) clearTimeout(copiedTimer.current)
    }
  }, [])
  return (
    <div className="relative">
      <pre
        className="aurora-scrollbar m-0 max-h-[150px] overflow-auto whitespace-pre-wrap break-words rounded-lg border px-2.5 py-2 font-mono text-[11px] leading-relaxed text-aurora-text-primary"
        style={{
          background: 'color-mix(in srgb, var(--aurora-page-bg) 55%, var(--aurora-control-surface))',
          borderColor: 'color-mix(in srgb, var(--aurora-border-default) 50%, var(--aurora-page-bg))',
        }}
      >
        {value}
      </pre>
      <button
        type="button"
        aria-label="Copy"
        title="Copy"
        onClick={() => {
          void navigator.clipboard
            ?.writeText(value)
            .then(() => {
              if (cancelledRef.current) return
              setCopied(true)
              if (copiedTimer.current !== null) clearTimeout(copiedTimer.current)
              copiedTimer.current = setTimeout(() => {
                copiedTimer.current = null
                setCopied(false)
              }, 1200)
            })
            .catch(() => {})
        }}
        className="absolute right-1.5 top-1.5 flex size-5 cursor-pointer items-center justify-center rounded border border-transparent text-aurora-text-muted transition-colors hover:border-aurora-border-strong hover:text-aurora-text-primary"
      >
        {copied ? (
          <Check className="size-3 text-aurora-success" strokeWidth={2} />
        ) : (
          <Copy className="size-3" strokeWidth={1.75} />
        )}
      </button>
    </div>
  )
}
