'use client'

import { type CSSProperties, useCallback, useEffect, useState } from 'react'
import {
  AlertTriangle,
  Check,
  ChevronRight,
  CornerDownLeft,
  History,
  Terminal,
  Wrench,
  X,
} from 'lucide-react'

import { AURORA_BADGE_LABEL } from '@/components/aurora/tokens'
import {
  type CodeModeCallTrace,
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
        connect: () => Promise<unknown>
        close?: () => Promise<unknown> | void
      }
    }
  }
}

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

function stateFromInitialTrace(initialTrace: unknown): InspectorState {
  const trace = parseCodeModeTrace(initialTrace)
  return trace ? applyTrace(emptyState(), trace) : emptyState()
}

export function CodeModeInspector({ initialTrace }: CodeModeInspectorProps) {
  const [state, setState] = useState<InspectorState>(() => stateFromInitialTrace(initialTrace))
  const [expanded, setExpanded] = useState<Record<string, boolean>>({})
  const [toolInput, setToolInput] = useState<unknown>(null)
  const [bridgeWarning, setBridgeWarning] = useState<string | null>(null)
  const [bridgeState, setBridgeState] = useState<'connecting' | 'connected' | 'fallback'>('fallback')

  const acceptTrace = useCallback((raw: unknown): boolean => {
    const trace = parseCodeModeTrace(raw)
    if (!trace) return false
    setState((previous) => applyTrace(previous, trace))
    setExpanded({})
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
        // Host explicitly cleared the result — drop the stale trace.
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
  const runOk = live ? calls.every((call) => call.ok) : (selectedEntry?.ok ?? true)
  const errorKind = live
    ? calls.find((call) => !call.ok)?.error_kind
    : selectedEntry?.error_kind
  const elapsedMs = live
    ? state.history.find(
        (entry) => entry.execution_id !== undefined && entry.execution_id === live.execution_id,
      )?.elapsed_ms
    : selectedEntry?.elapsed_ms
  const tokens = live ?? selectedEntry
  const discovery = live ? parseDiscoveryResult(live.result) : null
  const describeDoc = live ? describeMarkdown(live.result) : null
  // Host-delivered tool-call arguments apply to the live run only.
  const inputSnippet = live ? toolInputSnippet(toolInput) : null
  const warnings = [
    ...(bridgeWarning ? [bridgeWarning] : []),
    ...(state.live?.warnings?.map((warning) => warning.message) ?? []),
    ...state.historyWarnings,
  ]

  return (
    <main
      className="min-h-[100dvh] bg-aurora-page-bg p-4 font-sans text-aurora-text-primary"
      style={{
        ...AURORA_DARK_TOKENS,
        background:
          'radial-gradient(900px 420px at 12% -10%, rgba(41,182,246,0.08), transparent 60%), var(--aurora-page-bg)',
      }}
    >
      <section
        className="mx-auto w-full max-w-[680px] overflow-hidden rounded-[18px] border"
        style={{
          background: 'linear-gradient(180deg, var(--aurora-panel-strong-top), var(--aurora-panel-strong))',
          borderColor: 'color-mix(in srgb, var(--aurora-border-default) 45%, var(--aurora-page-bg))',
          boxShadow: 'var(--aurora-shadow-medium), var(--aurora-highlight-strong)',
        }}
      >
        <WidgetHead
          subLabel={
            !run
              ? null
              : discovery
                ? `${discovery.hits.length} of ${discovery.total} match${discovery.total === 1 ? '' : 'es'}`
                : describeDoc
                  ? 'describe'
                  : `${calls.length} call${calls.length === 1 ? '' : 's'}`
          }
          ok={runOk}
          errorKind={errorKind}
          elapsedMs={elapsedMs}
          bridgeState={bridgeState}
        />

        {warnings.map((warning, index) => (
          <WarnLine key={`${warning}-${index}`} message={warning} />
        ))}

        {!run ? (
          <p className="px-3 py-4 text-center text-xs text-aurora-text-muted">
            Waiting for an MCP Apps tool result or history snapshot.
          </p>
        ) : (
          <div>
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
            {selectedEntry ? <HistoryNote /> : null}
          </div>
        )}

        <WidgetFoot
          history={state.history}
          live={state.live}
          selected={state.selected}
          onSelect={(selection) => {
            setState((previous) => ({ ...previous, selected: selection }))
            setExpanded({})
          }}
          inputTokens={tokens?.input_tokens}
          outputTokens={tokens?.output_tokens}
          logsCount={live?.logs_count}
        />
      </section>
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
  subLabel,
  ok,
  errorKind,
  elapsedMs,
  bridgeState,
}: {
  subLabel: string | null
  ok: boolean
  errorKind: string | undefined
  elapsedMs: number | undefined
  bridgeState: 'connecting' | 'connected' | 'fallback'
}) {
  return (
    <div
      className="flex items-center gap-2 border-b px-3 py-2"
      style={{ borderColor: HEAD_FOOT_BORDER, background: HEAD_FOOT_BG }}
    >
      <LabbyMark />
      <span className="text-[12.5px] font-bold">Execute</span>
      {subLabel !== null ? (
        <span className="truncate text-[11.5px] text-aurora-text-muted">· {subLabel}</span>
      ) : null}
      <span className="flex-1" />
      {subLabel !== null ? (
        ok ? (
          <StatusDot tone="success" label="success" />
        ) : (
          <span className={cn(AURORA_BADGE_LABEL, 'text-aurora-error')}>{errorKind ?? 'error'}</span>
        )
      ) : null}
      {elapsedMs !== undefined ? (
        <span className="text-[11px] font-semibold tabular-nums text-aurora-text-muted">
          {formatMs(elapsedMs)}
        </span>
      ) : null}
      {bridgeState !== 'connected' ? (
        <span className={cn(AURORA_BADGE_LABEL, 'text-aurora-text-muted')}>{bridgeState}</span>
      ) : null}
    </div>
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
  return (
    <div>
      {calls.map((call, index) => {
        const key = `call:${call.id}-${index}`
        const open = Boolean(expanded[key])
        const params = stringifyRedactedParams(call.params)
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
                  className="absolute inset-y-0 left-0 min-w-1 rounded-full"
                  style={{
                    width: `${Math.max((call.elapsed_ms / maxElapsed) * 100, 4).toFixed(1)}%`,
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
        <span className="truncate text-[11px] text-aurora-text-muted">{shape}</span>
        <ChevronRight
          className={cn('size-3 text-aurora-text-muted transition-transform', open && 'rotate-90')}
          strokeWidth={1.75}
        />
      </button>
      {open ? (
        <div className="px-3 pb-2 pl-[34px]">
          <CodeBlock value={markdown ?? stringifyRedactedParams(trace.result)} />
        </div>
      ) : null}
    </div>
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
  return (
    <pre
      className="aurora-scrollbar m-0 max-h-[150px] overflow-auto whitespace-pre-wrap break-words rounded-lg border px-2.5 py-2 font-mono text-[11px] leading-relaxed text-aurora-text-primary"
      style={{
        background: 'color-mix(in srgb, var(--aurora-page-bg) 55%, var(--aurora-control-surface))',
        borderColor: 'color-mix(in srgb, var(--aurora-border-default) 50%, var(--aurora-page-bg))',
      }}
    >
      {value}
    </pre>
  )
}

function WidgetFoot({
  history,
  live,
  selected,
  onSelect,
  inputTokens,
  outputTokens,
  logsCount,
}: {
  history: CodeModeHistoryEntry[]
  live: CodeModeExecuteTrace | null
  selected: RunSelection
  onSelect: (selection: RunSelection) => void
  inputTokens: number | undefined
  outputTokens: number | undefined
  logsCount: number | undefined
}) {
  const liveEntrySeq =
    live?.execution_id !== undefined
      ? history.find((entry) => entry.execution_id === live.execution_id)?.seq
      : undefined
  const chips: { key: string; label: string; ok: boolean; target: RunSelection }[] = history.map(
    (entry) => ({
      key: `seq-${entry.seq}`,
      label: entry.seq === liveEntrySeq ? `#${entry.seq} live` : `#${entry.seq}`,
      ok: entry.ok,
      target: entry.seq === liveEntrySeq ? 'live' : entry.seq,
    }),
  )
  if (live && liveEntrySeq === undefined) {
    chips.push({ key: 'live', label: 'live', ok: live.calls.every((call) => call.ok), target: 'live' })
  }

  const meta: string[] = []
  if (inputTokens !== undefined || outputTokens !== undefined) {
    meta.push(`${inputTokens ?? 0} in · ${outputTokens ?? 0} out`)
  }
  if (logsCount) meta.push(`${logsCount} log${logsCount === 1 ? '' : 's'}`)

  if (chips.length === 0 && meta.length === 0) return null

  return (
    <div
      className="flex items-center gap-1.5 border-t px-3 py-1.5"
      style={{ borderColor: HEAD_FOOT_BORDER, background: HEAD_FOOT_BG }}
    >
      <span className={cn(AURORA_BADGE_LABEL, 'mr-0.5 text-aurora-text-muted')}>Session</span>
      {chips.map((chip) => {
        const isSelected =
          chip.target === selected || (chip.target === 'live' && selected === 'live')
        return (
          <button
            key={chip.key}
            type="button"
            onClick={() => onSelect(chip.target)}
            className={cn(
              'inline-flex cursor-pointer items-center gap-1.5 rounded-full border px-2 py-0.5 text-[10.5px] font-semibold transition-colors',
              isSelected
                ? 'text-aurora-text-primary'
                : 'text-aurora-text-muted hover:text-aurora-text-primary',
            )}
            style={{
              background: isSelected
                ? 'color-mix(in srgb, var(--aurora-accent-primary) 8%, var(--aurora-control-surface))'
                : 'var(--aurora-control-surface)',
              borderColor: isSelected
                ? 'color-mix(in srgb, var(--aurora-accent-primary) 55%, var(--aurora-border-strong))'
                : 'color-mix(in srgb, var(--aurora-border-default) 55%, var(--aurora-page-bg))',
              boxShadow: isSelected ? '0 0 0 1px rgba(41,182,246,0.24)' : undefined,
            }}
          >
            <span
              className={cn(
                'inline-block size-[5px] rounded-full',
                chip.ok ? 'bg-aurora-success' : 'bg-aurora-error',
              )}
            />
            {chip.label}
          </button>
        )
      })}
      <span className="flex-1" />
      {meta.length > 0 ? (
        <span className="text-[10.5px] tabular-nums text-aurora-text-muted">{meta.join(' · ')}</span>
      ) : null}
    </div>
  )
}
