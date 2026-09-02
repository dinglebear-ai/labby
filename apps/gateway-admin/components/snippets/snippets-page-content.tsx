'use client'

import * as React from 'react'
import { toast } from 'sonner'
import {
  ArrowDown,
  ArrowUp,
  FileCode2,
  FlaskConical,
  Loader2,
  Package,
  Plus,
  Play,
  RefreshCw,
  Search,
  ShieldCheck,
  SlidersHorizontal,
  Trash2,
} from 'lucide-react'
import { ActionConfirmationDialog } from '@/components/action-confirmation-dialog'
import { AppHeader } from '@/components/app-header'
import { LibraryTabs } from '@/components/depot/depot-workspace-pages'
import { ConsoleHero } from '@/components/console/console-hero'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import { SafeMarkdown } from '@/components/markdown/safe-markdown'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { snippetsApi } from '@/lib/api/snippets-client'
import type { ResolvedSnippet, SnippetInfo, SnippetInputSpec } from '@/lib/types/snippets'
import {
  buildSnippetParams,
  collectSnippetTags,
  filterSnippets,
  inputPlaceholder,
  parseSnippetBody,
  snippetKey,
  sortSnippetsByName,
  tokenizeSnippetSource,
  type SnippetSortDirection,
  type SnippetTokenKind,
} from './snippet-model'

/**
 * Snippets screen body, measured off the Gateway Console mock.
 *
 * The mock draws one `--radius-2` card holding a filter row, a six-column
 * snippet table, and an inline detail region that expands under the selected
 * row. Everything in here is sized/coloured from that mock's live DOM.
 *
 * Four of the mock's columns — SERVERS, RUNS, FAILS, AVG — and the HISTORY
 * sparkline have no field on `SnippetInfo`, so they render `—`. The mock does
 * the same wherever its own fixture is missing a value; nothing is invented.
 * Upstream servers *are* recoverable for the selected snippet, because its
 * resolved body is fetched — they surface as the TOOLS CALLED chips.
 */

// ---------------------------------------------------------------------------
// Measured chrome
// ---------------------------------------------------------------------------

const CARD: React.CSSProperties = {
  borderRadius: 'var(--radius-2)',
  border: '1px solid color-mix(in srgb, var(--aurora-border-default) 45%, var(--aurora-page-bg))',
  background: 'linear-gradient(180deg, var(--aurora-panel-strong-top), var(--aurora-panel-strong))',
  boxShadow: 'var(--aurora-shadow-strong), inset 0 1px 0 rgba(255,255,255,0.05)',
  overflow: 'hidden',
}

const GRID_COLUMNS = 'minmax(240px, 1.4fr) minmax(140px, 1fr) 70px 60px 70px 90px 40px'

const FILTER_ROW: React.CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  gap: 10,
  padding: '10px 14px',
  borderBottom: '1px solid color-mix(in srgb, var(--aurora-border-default) 70%, var(--aurora-page-bg))',
  background: 'var(--gw0-0_38)',
}

const HEAD_ROW: React.CSSProperties = {
  display: 'grid',
  gridTemplateColumns: GRID_COLUMNS,
  alignItems: 'center',
  gap: 8,
  padding: '0 14px',
  height: 34,
  borderBottom: '1px solid var(--aurora-border-strong)',
  background: 'var(--gw0-0_48)',
}

const HEAD_LABEL: React.CSSProperties = {
  fontSize: 10,
  fontWeight: 700,
  letterSpacing: '0.14em',
  textTransform: 'uppercase',
  color: 'var(--aurora-text-muted)',
}

const CONTROL: React.CSSProperties = {
  border: '1px solid color-mix(in srgb, var(--aurora-border-default) 70%, var(--aurora-page-bg))',
  background: 'var(--aurora-control-surface)',
  outline: 'none',
  fontFamily: 'inherit',
  color: 'var(--aurora-text-primary)',
}

const SECTION_LABEL: React.CSSProperties = {
  fontSize: 9.5,
  fontWeight: 700,
  letterSpacing: '0.14em',
  textTransform: 'uppercase',
  color: 'color-mix(in srgb, var(--aurora-text-muted) 75%, transparent)',
  marginBottom: 7,
}

const CODE_BLOCK: React.CSSProperties = {
  margin: 0,
  padding: '12px 14px',
  maxHeight: 340,
  borderRadius: 9,
  border: '1px solid color-mix(in srgb, var(--aurora-border-default) 50%, var(--aurora-page-bg))',
  background: 'var(--gw4-0_62)',
  fontFamily: "'JetBrains Mono', var(--font-mono)",
  fontSize: 11,
  lineHeight: 1.75,
  overflow: 'auto',
  whiteSpace: 'pre-wrap',
  wordBreak: 'break-word',
}

const TOKEN_COLOR: Record<SnippetTokenKind, string | undefined> = {
  plain: undefined,
  comment: 'color-mix(in srgb, var(--aurora-text-muted) 65%, transparent)',
  meta: 'color-mix(in srgb, var(--aurora-text-muted) 65%, transparent)',
  string: 'var(--aurora-success)',
  keyword: 'var(--aurora-accent-pink)',
  key: 'var(--aurora-accent-strong)',
}

function rowStyle(index: number, selected: boolean): React.CSSProperties {
  const zebra = index % 2 === 0 ? 'var(--gw1-0_62)' : 'var(--gw2-0_55)'
  return {
    display: 'grid',
    gridTemplateColumns: GRID_COLUMNS,
    alignItems: 'center',
    gap: 8,
    padding: '9px 14px',
    cursor: 'pointer',
    borderTop: '1px solid color-mix(in srgb, var(--aurora-border-default) 55%, var(--aurora-page-bg))',
    transition: 'background 150ms',
    background: selected
      ? `color-mix(in srgb, var(--aurora-accent-primary) 6%, ${zebra})`
      : zebra,
    boxShadow: selected
      ? 'inset 3px 0 0 color-mix(in srgb, var(--aurora-accent-primary) 42%, transparent)'
      : undefined,
  }
}

/** The mock's stand-in for a value its fixture does not carry. */
function MissingCell({ title, align = 'center' }: { title: string; align?: 'start' | 'center' }) {
  return (
    <span
      title={title}
      style={{
        justifySelf: align,
        fontSize: 11.5,
        fontVariantNumeric: 'tabular-nums',
        color: 'color-mix(in srgb, var(--aurora-text-muted) 55%, transparent)',
      }}
    >
      —
    </span>
  )
}

function SectionLabel({ children }: { children: React.ReactNode }) {
  return <div style={SECTION_LABEL}>{children}</div>
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

type ActionState =
  | { kind: 'idle' }
  | { kind: 'loading'; label: string }
  | { kind: 'success'; label: string; detail: string }
  | { kind: 'error'; label: string; detail: string }

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : 'Unknown snippets error'
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

function actionResultFailed(result: unknown): boolean {
  if (!isObject(result)) return false
  if (typeof result.valid === 'boolean') return !result.valid
  if (typeof result.passed === 'boolean') return !result.passed
  if (Array.isArray(result.results)) {
    return result.results.some((entry) => isObject(entry) && entry.passed === false)
  }
  if (isObject(result.result) && typeof result.result.ok === 'boolean') return !result.result.ok
  return false
}

function inputEntries(snippet: SnippetInfo | null): Array<[string, SnippetInputSpec]> {
  return Object.entries(snippet?.inputs ?? {})
}

export function SnippetsPageContent() {
  const [snippets, setSnippets] = React.useState<SnippetInfo[]>([])
  const [selectedKey, setSelectedKey] = React.useState<string | null>(null)
  const [selectedDetail, setSelectedDetail] = React.useState<ResolvedSnippet | null>(null)
  const [detailError, setDetailError] = React.useState<string | null>(null)
  const [loading, setLoading] = React.useState(true)
  const [error, setError] = React.useState<string | null>(null)
  const [actionState, setActionState] = React.useState<ActionState>({ kind: 'idle' })
  const [query, setQuery] = React.useState('')
  const [activeTag, setActiveTag] = React.useState<string | null>(null)
  const [sortDirection, setSortDirection] = React.useState<SnippetSortDirection>('asc')
  const [inputValues, setInputValues] = React.useState<Record<string, Record<string, string>>>({})
  const [createOpen, setCreateOpen] = React.useState(false)
  const [createName, setCreateName] = React.useState('')
  const [createDescription, setCreateDescription] = React.useState('')
  const [createBody, setCreateBody] = React.useState('async () => {\n  return { ok: true }\n}')
  const [createError, setCreateError] = React.useState<string | null>(null)
  const [creating, setCreating] = React.useState(false)
  const [removeConfirmKey, setRemoveConfirmKey] = React.useState<string | null>(null)
  const [removing, setRemoving] = React.useState(false)

  const reload = React.useCallback(async () => {
    setLoading(true)
    try {
      const next = await snippetsApi.list()
      setSnippets(next)
      setSelectedKey((current) => {
        if (current && next.some((snippet) => snippetKey(snippet) === current)) return current
        return next[0] ? snippetKey(next[0]) : null
      })
      setError(null)
    } catch (err) {
      setError(errorMessage(err))
    } finally {
      setLoading(false)
    }
  }, [])

  React.useEffect(() => {
    const controller = new AbortController()
    setLoading(true)
    snippetsApi
      .list(controller.signal)
      .then((next) => {
        setSnippets(next)
        setSelectedKey(next[0] ? snippetKey(next[0]) : null)
        setError(null)
      })
      .catch((err) => {
        if (!controller.signal.aborted) setError(errorMessage(err))
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false)
      })
    return () => controller.abort()
  }, [])

  React.useEffect(() => {
    const selected = snippets.find((snippet) => snippetKey(snippet) === selectedKey) ?? null
    if (!selected) {
      setSelectedDetail(null)
      setDetailError(null)
      return
    }

    const controller = new AbortController()
    snippetsApi
      .get(selected.name, controller.signal)
      .then((detail) => {
        setSelectedDetail(detail)
        setDetailError(null)
      })
      .catch((err) => {
        if (controller.signal.aborted) return
        setSelectedDetail(null)
        setDetailError(errorMessage(err))
      })

    return () => controller.abort()
  }, [selectedKey, snippets])

  const tags = React.useMemo(() => collectSnippetTags(snippets), [snippets])
  const visible = React.useMemo(
    () => sortSnippetsByName(filterSnippets(snippets, query, activeTag), sortDirection),
    [snippets, query, activeTag, sortDirection],
  )

  const selected = React.useMemo(
    () => snippets.find((snippet) => snippetKey(snippet) === selectedKey) ?? null,
    [selectedKey, snippets],
  )
  const selectedDetailLoaded = selectedDetail?.name === selected?.name
  const parsed = React.useMemo(
    () => parseSnippetBody(selectedDetailLoaded ? selectedDetail?.body : null),
    [selectedDetail, selectedDetailLoaded],
  )
  const sourceTokens = React.useMemo(() => tokenizeSnippetSource(parsed.source), [parsed.source])

  const builtinCount = snippets.filter((snippet) => snippet.source === 'builtin').length
  const inputCount = snippets.reduce((sum, snippet) => sum + inputEntries(snippet).length, 0)

  const runAction = async (label: string, fn: () => Promise<unknown>) => {
    setActionState({ kind: 'loading', label })
    try {
      const result = await fn()
      const detail =
        typeof result === 'object' && result !== null
          ? JSON.stringify(result, null, 2).slice(0, 2000)
          : String(result)
      setActionState({ kind: actionResultFailed(result) ? 'error' : 'success', label, detail })
    } catch (err) {
      setActionState({ kind: 'error', label, detail: errorMessage(err) })
    }
  }

  /** Coerce the INPUTS table into typed params before test/exec. */
  const withParams = (
    snippet: SnippetInfo,
    label: string,
    fn: (params: Record<string, unknown>) => Promise<unknown>,
  ) => {
    const built = buildSnippetParams(snippet.inputs, inputValues[snippetKey(snippet)])
    if (!built.ok) {
      setActionState({ kind: 'error', label, detail: built.error })
      return
    }
    void runAction(label, () => fn(built.params))
  }

  const setInputValue = (key: string, name: string, value: string) => {
    setInputValues((current) => ({ ...current, [key]: { ...current[key], [name]: value } }))
  }

  const running = actionState.kind === 'loading' ? actionState.label : null

  const createSnippet = async () => {
    const name = createName.trim()
    if (!name) {
      setCreateError('Name is required.')
      return
    }
    if (!createBody.trim()) {
      setCreateError('Snippet body is required.')
      return
    }

    setCreating(true)
    setCreateError(null)
    try {
      const created = await snippetsApi.create({
        name,
        body: createBody,
        ...(createDescription.trim() ? { description: createDescription.trim() } : {}),
      })
      await reload()
      setSelectedKey(snippetKey(created))
      setCreateOpen(false)
      setCreateName('')
      setCreateDescription('')
      setCreateBody('async () => {\n  return { ok: true }\n}')
      setActionState({ kind: 'success', label: 'Create', detail: `Created ${created.name}` })
    } catch (err) {
      setCreateError(errorMessage(err))
    } finally {
      setCreating(false)
    }
  }

  const removeConfirmSnippet = removeConfirmKey
    ? (snippets.find((snippet) => snippetKey(snippet) === removeConfirmKey) ?? null)
    : null

  // Removal reports through a toast, not `actionState`. Every other action on
  // this page reports inside the selected snippet's detail row, which is right
  // for them because their subject still exists afterwards. Removal destroys
  // its own subject: `reload` moves the selection to a different snippet, or to
  // null when that was the last one — so a detail-row message lands under an
  // unrelated snippet or renders nowhere at all. A toast also sidesteps the
  // original bug directly, since it portals above the confirmation dialog's
  // modal overlay instead of behind it. This matches how the gateways page
  // reports the same action (`toast.success('Server removed successfully')`).
  const confirmRemove = async () => {
    const snippet = removeConfirmSnippet
    if (!snippet) {
      setRemoveConfirmKey(null)
      return
    }
    setRemoving(true)
    try {
      const result = await snippetsApi.remove(snippet.name)
      // The backend signals refusal by throwing, so `removed: false` is not
      // reachable today — but reporting "Removed X" without looking would be
      // the same unverified-success bug this page's Save flow already had.
      // Read it before `reload`, which can fail on its own and must not be
      // able to turn a completed removal into a "Remove failed".
      const removed = result.removed
      await reload()
      if (removed) {
        toast.success(`Removed ${snippet.name}`)
      } else {
        toast.error(`${snippet.name} was not removed.`)
      }
    } catch (err) {
      toast.error(`Remove failed: ${errorMessage(err)}`)
    } finally {
      setRemoveConfirmKey(null)
      setRemoving(false)
    }
  }

  return (
    <>
      <AppHeader
        breadcrumbs={[{ label: 'Depot' }, { label: 'Library' }, { label: 'Snippets' }]}
        actions={
          <Button size="sm" onClick={() => setCreateOpen(true)}>
            <Plus className="size-4" />
            New snippet
          </Button>
        }
      />
      <div className={`${AURORA_PAGE_SHELL} flex-1`}>
        <div className={AURORA_PAGE_FRAME}>
          <LibraryTabs active="snippets" />
          {/* Hero — the mock's eyebrow + title + action cluster with the stat
              strip welded to the card's bottom edge, not floating cards. */}
          <ConsoleHero
            eyebrow="Code Mode"
            title="Snippets"
            actions={
              <>
                <Button size="sm" onClick={() => setCreateOpen(true)}>
                  <Plus className="size-4" />
                  New snippet
                </Button>
                <Button variant="outline" size="sm" onClick={() => void reload()}>
                  <RefreshCw className="size-4" />
                  Refresh
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => void runAction('Test all', () => snippetsApi.testAll())}
                  disabled={snippets.length === 0}
                >
                  <FlaskConical className="size-4" />
                  Test all
                </Button>
              </>
            }
            stats={[
              { label: 'Snippets', value: snippets.length, icon: <FileCode2 size={12} strokeWidth={1.8} /> },
              { label: 'Built-in', value: builtinCount, icon: <Package size={12} strokeWidth={1.8} /> },
              { label: 'Inputs', value: inputCount, icon: <SlidersHorizontal size={12} strokeWidth={1.8} /> },
            ]}
          />

          <section style={CARD}>
            {/* Filter row: search, tag pills, right-aligned count. */}
            <div style={FILTER_ROW}>
              <div style={{ flex: '1 1 0%', maxWidth: 340, position: 'relative' }}>
                <span
                  style={{
                    position: 'absolute',
                    left: 10,
                    top: '50%',
                    transform: 'translateY(-50%)',
                    color: 'var(--aurora-text-muted)',
                    display: 'grid',
                    placeItems: 'center',
                  }}
                >
                  <Search size={12} strokeWidth={1.8} />
                </span>
                <input
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  placeholder="Search snippets, tools, tags…"
                  aria-label="Search snippets"
                  style={{
                    ...CONTROL,
                    width: '100%',
                    height: 30,
                    padding: '0 10px 0 30px',
                    borderRadius: 9,
                    fontSize: 12,
                    boxSizing: 'border-box',
                  }}
                />
              </div>

              <div style={{ display: 'flex', gap: 4, flexWrap: 'wrap' }}>
                {tags.map((tag) => {
                  const active = activeTag === tag
                  return (
                    <button
                      key={tag}
                      type="button"
                      aria-pressed={active}
                      onClick={() => setActiveTag(active ? null : tag)}
                      style={{
                        height: 24,
                        padding: '0 10px',
                        borderRadius: 999,
                        fontFamily: 'inherit',
                        fontSize: 10.5,
                        fontWeight: 650,
                        cursor: 'pointer',
                        transition: 'background 150ms, border-color 150ms, color 150ms',
                        border: active
                          ? '1px solid color-mix(in srgb, var(--aurora-accent-primary) 45%, transparent)'
                          : '1px solid color-mix(in srgb, var(--aurora-border-default) 70%, var(--aurora-page-bg))',
                        background: active
                          ? 'color-mix(in srgb, var(--aurora-accent-primary) 13%, transparent)'
                          : 'var(--aurora-control-surface)',
                        color: active ? 'var(--aurora-accent-strong)' : 'var(--aurora-text-muted)',
                      }}
                    >
                      {tag}
                    </button>
                  )
                })}
              </div>

              <div style={{ flex: '1 1 0%' }} />

              <span
                style={{
                  fontSize: 11,
                  color: 'var(--aurora-text-muted)',
                  fontVariantNumeric: 'tabular-nums',
                  whiteSpace: 'nowrap',
                }}
              >
                {visible.length} of {snippets.length} snippets
              </span>
            </div>

            {/* Table head. Only SNIPPET is sortable — the remaining columns have
                no data behind them, so there is nothing to order by. */}
            <div style={HEAD_ROW}>
              <button
                type="button"
                onClick={() => setSortDirection((current) => (current === 'asc' ? 'desc' : 'asc'))}
                style={{
                  ...HEAD_LABEL,
                  justifySelf: 'start',
                  display: 'inline-flex',
                  alignItems: 'center',
                  gap: 4,
                  border: 'none',
                  background: 'none',
                  padding: 0,
                  cursor: 'pointer',
                  fontFamily: 'inherit',
                }}
              >
                Snippet
                {sortDirection === 'asc' ? <ArrowUp size={10} /> : <ArrowDown size={10} />}
              </button>
              <span style={HEAD_LABEL}>Servers</span>
              <span style={{ ...HEAD_LABEL, justifySelf: 'center' }}>Runs</span>
              <span style={{ ...HEAD_LABEL, justifySelf: 'center' }}>Fails</span>
              <span style={{ ...HEAD_LABEL, justifySelf: 'center' }}>Avg</span>
              <span style={{ ...HEAD_LABEL, justifySelf: 'center' }}>History</span>
              <span />
            </div>

            {error ? (
              <div
                style={{
                  padding: '14px 16px',
                  fontSize: 12,
                  color: 'var(--aurora-error)',
                  background: 'var(--gw1-0_62)',
                }}
              >
                Failed to load snippets: {error}
              </div>
            ) : loading && snippets.length === 0 ? (
              Array.from({ length: 5 }, (_, index) => (
                <div
                  key={index}
                  className="animate-pulse"
                  style={{ ...rowStyle(index, false), height: 43, cursor: 'default' }}
                >
                  <span
                    style={{
                      height: 10,
                      borderRadius: 4,
                      background: 'color-mix(in srgb, var(--aurora-text-muted) 18%, transparent)',
                    }}
                  />
                </div>
              ))
            ) : visible.length === 0 ? (
              <div
                style={{
                  padding: '18px 16px',
                  fontSize: 12,
                  color: 'var(--aurora-text-muted)',
                  background: 'var(--gw1-0_62)',
                }}
              >
                {snippets.length === 0
                  ? 'No executable snippets found.'
                  : 'No snippets match this filter.'}
              </div>
            ) : (
              visible.map((snippet, index) => {
                const key = snippetKey(snippet)
                const isSelected = selected ? snippetKey(selected) === key : false
                return (
                  <React.Fragment key={key}>
                    <div
                      role="button"
                      tabIndex={0}
                      data-hoverrow="1"
                      aria-pressed={isSelected}
                      onClick={() => setSelectedKey(isSelected ? null : key)}
                      onKeyDown={(event) => {
                        if (event.key === 'Enter' || event.key === ' ') {
                          event.preventDefault()
                          setSelectedKey(isSelected ? null : key)
                        }
                      }}
                      style={rowStyle(index, isSelected)}
                    >
                      <div style={{ minWidth: 0, display: 'flex', alignItems: 'center', gap: 9 }}>
                        <span
                          title={
                            snippet.shadowed
                              ? 'Shadowed by a user snippet of the same name'
                              : `${snippet.source} snippet · ${snippet.path}`
                          }
                          style={{
                            flexShrink: 0,
                            width: 6,
                            height: 6,
                            borderRadius: 999,
                            background: snippet.shadowed
                              ? 'var(--aurora-warn)'
                              : 'color-mix(in srgb, var(--aurora-text-muted) 55%, transparent)',
                            boxShadow: snippet.shadowed ? '0 0 4px var(--aurora-warn)' : undefined,
                          }}
                        />
                        <span
                          style={{
                            minWidth: 0,
                            display: 'flex',
                            alignItems: 'baseline',
                            gap: 8,
                            overflow: 'hidden',
                          }}
                        >
                          <span
                            style={{
                              fontFamily: 'var(--font-display)',
                              fontSize: 12.5,
                              fontWeight: 760,
                              color: 'var(--aurora-text-primary)',
                              whiteSpace: 'nowrap',
                            }}
                          >
                            {snippet.name}
                          </span>
                          <span
                            style={{
                              minWidth: 0,
                              fontSize: 11,
                              color: 'var(--aurora-text-muted)',
                              whiteSpace: 'nowrap',
                              overflow: 'hidden',
                              textOverflow: 'ellipsis',
                            }}
                          >
                            {snippet.description ?? 'No description provided.'}
                          </span>
                        </span>
                      </div>

                      <MissingCell
                        align="start"
                        title="Servers — the snippets API does not report upstreams per snippet; select a row to see the tools its source calls"
                      />
                      <MissingCell title="Runs — the snippets API does not expose run counts" />
                      <MissingCell title="Fails — the snippets API does not expose failure counts" />
                      <MissingCell title="Avg — the snippets API does not expose runtimes" />
                      <MissingCell title="History — the snippets API does not expose per-run history" />

                      <button
                        type="button"
                        title="Execute now"
                        aria-label={`Execute ${snippet.name}`}
                        onClick={(event) => {
                          event.stopPropagation()
                          withParams(snippet, 'Execute', (params) =>
                            snippetsApi.exec(snippet.name, params),
                          )
                        }}
                        style={{
                          justifySelf: 'center',
                          display: 'grid',
                          placeItems: 'center',
                          width: 24,
                          height: 24,
                          borderRadius: 7,
                          border: '1px solid color-mix(in srgb, var(--aurora-accent-primary) 40%, transparent)',
                          background: 'color-mix(in srgb, var(--aurora-accent-primary) 10%, transparent)',
                          color: 'var(--aurora-accent-strong)',
                          cursor: 'pointer',
                        }}
                      >
                        <Play size={11} fill="currentColor" strokeWidth={0} />
                      </button>
                    </div>

                    {isSelected ? (
                      <div
                        style={{
                          borderTop:
                            '1px solid color-mix(in srgb, var(--aurora-accent-primary) 20%, var(--aurora-border-default))',
                          boxShadow:
                            'inset 3px 0 0 color-mix(in srgb, var(--aurora-accent-primary) 42%, transparent)',
                        }}
                      >
                        <div
                          style={{
                            display: 'flex',
                            alignItems: 'center',
                            gap: 6,
                            flexWrap: 'wrap',
                            padding: '10px 16px',
                            borderBottom:
                              '1px solid color-mix(in srgb, var(--aurora-border-default) 55%, var(--aurora-page-bg))',
                            background: 'var(--gw0-0_30)',
                          }}
                        >
                          {(snippet.tags ?? []).map((tag) => (
                            <span
                              key={tag}
                              style={{
                                display: 'inline-flex',
                                alignItems: 'center',
                                height: 19,
                                padding: '0 8px',
                                borderRadius: 6,
                                border:
                                  '1px solid color-mix(in srgb, var(--aurora-border-strong) 80%, transparent)',
                                background: 'var(--gw0-0_48)',
                                fontSize: 9.5,
                                fontWeight: 650,
                                letterSpacing: '0.06em',
                                textTransform: 'uppercase',
                                color: 'var(--aurora-text-muted)',
                              }}
                            >
                              {tag}
                            </span>
                          ))}
                          <div style={{ flex: '1 1 0%' }} />
                          <DetailButton
                            label="Validate"
                            icon={<ShieldCheck size={11} />}
                            busy={running === 'Validate'}
                            disabled={running !== null}
                            onClick={() =>
                              void runAction('Validate', () => snippetsApi.validate(snippet.name))
                            }
                          />
                          <DetailButton
                            label="Test"
                            icon={<FlaskConical size={11} />}
                            busy={running === 'Test'}
                            disabled={running !== null}
                            onClick={() =>
                              withParams(snippet, 'Test', (params) =>
                                snippetsApi.test(snippet.name, params),
                              )
                            }
                          />
                          <DetailButton
                            label="Execute"
                            icon={<Play size={11} fill="currentColor" strokeWidth={0} />}
                            busy={running === 'Execute'}
                            disabled={running !== null}
                            primary
                            onClick={() =>
                              withParams(snippet, 'Execute', (params) =>
                                snippetsApi.exec(snippet.name, params),
                              )
                            }
                          />
                          {snippet.source !== 'builtin' ? (
                            <DetailButton
                              label="Remove"
                              icon={<Trash2 size={11} />}
                              disabled={running !== null}
                              danger
                              onClick={() => setRemoveConfirmKey(snippetKey(snippet))}
                            />
                          ) : null}
                        </div>

                        <div
                          style={{
                            padding: '14px 16px',
                            display: 'flex',
                            flexDirection: 'column',
                            gap: 14,
                          }}
                        >
                          <div>
                            <SectionLabel>Tools called</SectionLabel>
                            {detailError ? (
                              <p style={{ margin: 0, fontSize: 11.5, color: 'var(--aurora-error)' }}>
                                Failed to load snippet body: {detailError}
                              </p>
                            ) : !selectedDetailLoaded ? (
                              <p style={{ margin: 0, fontSize: 11.5, color: 'var(--aurora-text-muted)' }}>
                                Loading source…
                              </p>
                            ) : parsed.tools.length === 0 ? (
                              <p style={{ margin: 0, fontSize: 11.5, color: 'var(--aurora-text-muted)' }}>
                                No <code>callTool</code> references found in this snippet&apos;s source.
                              </p>
                            ) : (
                              <div style={{ display: 'flex', gap: 5, flexWrap: 'wrap' }}>
                                {parsed.tools.map((tool) => (
                                  <span
                                    key={tool}
                                    style={{
                                      display: 'inline-flex',
                                      alignItems: 'center',
                                      gap: 5,
                                      height: 24,
                                      padding: '0 9px',
                                      borderRadius: 8,
                                      border:
                                        '1px solid color-mix(in srgb, var(--aurora-accent-primary) 24%, transparent)',
                                      background:
                                        'color-mix(in srgb, var(--aurora-accent-primary) 8%, transparent)',
                                      fontSize: 11,
                                      color: 'var(--aurora-accent-strong)',
                                    }}
                                  >
                                    {tool}
                                  </span>
                                ))}
                              </div>
                            )}
                          </div>

                          <div>
                            <SectionLabel>Inputs</SectionLabel>
                            {inputEntries(snippet).length === 0 ? (
                              <p style={{ margin: 0, fontSize: 11.5, color: 'var(--aurora-text-muted)' }}>
                                This snippet does not declare typed inputs.
                              </p>
                            ) : (
                              <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                                {inputEntries(snippet).map(([name, spec]) => (
                                  <div key={name}>
                                    <div
                                      style={{
                                        display: 'grid',
                                        gridTemplateColumns: '140px 90px minmax(0px, 1fr)',
                                        gap: 10,
                                        alignItems: 'center',
                                      }}
                                    >
                                      <span style={{ fontSize: 11.5, color: 'var(--aurora-text-primary)' }}>
                                        {name}
                                      </span>
                                      <span style={{ fontSize: 10.5, color: 'var(--aurora-text-muted)' }}>
                                        {spec.ty}
                                        {spec.required ? ' *' : ''}
                                      </span>
                                      <input
                                        aria-label={name}
                                        placeholder={inputPlaceholder(spec)}
                                        value={inputValues[key]?.[name] ?? ''}
                                        onChange={(event) => setInputValue(key, name, event.target.value)}
                                        style={{
                                          ...CONTROL,
                                          height: 30,
                                          padding: '0 10px',
                                          borderRadius: 8,
                                          fontSize: 11,
                                        }}
                                      />
                                    </div>
                                    {spec.description ? (
                                      <p
                                        style={{
                                          margin: '3px 0 0',
                                          fontSize: 10.5,
                                          color: 'color-mix(in srgb, var(--aurora-text-muted) 80%, transparent)',
                                        }}
                                      >
                                        {spec.description}
                                      </p>
                                    ) : null}
                                  </div>
                                ))}
                              </div>
                            )}
                          </div>

                          {/* Deliberate addition: the mock has no tutorial region,
                              but built-in snippets ship rendered walkthroughs and
                              dropping them would lose real functionality. */}
                          {selectedDetailLoaded && parsed.tutorial ? (
                            <div>
                              <SectionLabel>Tutorial</SectionLabel>
                              <div
                                style={{
                                  ...CODE_BLOCK,
                                  fontFamily: 'inherit',
                                  fontSize: 12,
                                  lineHeight: 1.6,
                                  whiteSpace: 'normal',
                                }}
                              >
                                <SafeMarkdown
                                  text={parsed.tutorial}
                                  className="text-aurora-text-muted [&_h1]:font-display [&_h1]:text-[17px] [&_h1]:font-extrabold [&_h1]:text-aurora-text-primary [&_h2]:mt-4 [&_h2]:font-display [&_h2]:text-[14px] [&_h2]:font-bold [&_h2]:text-aurora-text-primary [&_h3]:mt-3 [&_h3]:font-semibold [&_h3]:text-aurora-text-primary [&_li]:my-1 [&_ol]:my-2 [&_ol]:list-decimal [&_ol]:pl-5 [&_p]:my-2 [&_pre]:my-2 [&_pre]:overflow-auto [&_pre]:rounded-[8px] [&_pre]:border [&_pre]:border-aurora-border-strong [&_pre]:bg-aurora-control-surface [&_pre]:p-2.5 [&_table]:my-2 [&_td]:border [&_td]:border-aurora-border-strong [&_td]:px-2 [&_td]:py-1 [&_th]:border [&_th]:border-aurora-border-strong [&_th]:px-2 [&_th]:py-1 [&_ul]:my-2 [&_ul]:list-disc [&_ul]:pl-5"
                                />
                              </div>
                            </div>
                          ) : null}

                          <div>
                            <SectionLabel>Source</SectionLabel>
                            {detailError ? (
                              <p style={{ margin: 0, fontSize: 11.5, color: 'var(--aurora-error)' }}>
                                Failed to load snippet body: {detailError}
                              </p>
                            ) : !selectedDetailLoaded ? (
                              <p style={{ margin: 0, fontSize: 11.5, color: 'var(--aurora-text-muted)' }}>
                                Loading source…
                              </p>
                            ) : (
                              <pre style={CODE_BLOCK}>
                                {sourceTokens.map((token, tokenIndex) => (
                                  <span
                                    key={tokenIndex}
                                    style={{ color: TOKEN_COLOR[token.kind] }}
                                  >
                                    {token.text}
                                  </span>
                                ))}
                              </pre>
                            )}
                          </div>

                          {actionState.kind === 'idle' ? null : (
                            <div>
                              <SectionLabel>
                                {actionState.label}{' '}
                                {actionState.kind === 'loading'
                                  ? 'running'
                                  : actionState.kind === 'success'
                                    ? 'completed'
                                    : 'failed'}
                              </SectionLabel>
                              {actionState.kind === 'loading' ? (
                                <p
                                  style={{
                                    margin: 0,
                                    display: 'flex',
                                    alignItems: 'center',
                                    gap: 6,
                                    fontSize: 11.5,
                                    color: 'var(--aurora-text-muted)',
                                  }}
                                >
                                  <Loader2 className="animate-spin" size={12} />
                                  Running {actionState.label.toLowerCase()}…
                                </p>
                              ) : (
                                <pre
                                  style={{
                                    ...CODE_BLOCK,
                                    maxHeight: 240,
                                    color:
                                      actionState.kind === 'error'
                                        ? 'var(--aurora-error)'
                                        : 'var(--aurora-text-muted)',
                                  }}
                                >
                                  {actionState.detail}
                                </pre>
                              )}
                            </div>
                          )}
                        </div>
                      </div>
                    ) : null}
                  </React.Fragment>
                )
              })
            )}
          </section>
        </div>
      </div>
      <Dialog open={createOpen} onOpenChange={setCreateOpen}>
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>Create snippet</DialogTitle>
            <DialogDescription>
              Save an executable Code Mode snippet under your Labby home.
            </DialogDescription>
          </DialogHeader>
          <div className="grid gap-4 py-2">
            <div className="grid gap-2">
              <Label htmlFor="snippet-name">Name</Label>
              <Input
                id="snippet-name"
                value={createName}
                onChange={(event) => setCreateName(event.target.value)}
                placeholder="fleet-health"
                autoComplete="off"
              />
            </div>
            <div className="grid gap-2">
              <Label htmlFor="snippet-description">Description</Label>
              <Input
                id="snippet-description"
                value={createDescription}
                onChange={(event) => setCreateDescription(event.target.value)}
                placeholder="What this workflow does"
              />
            </div>
            <div className="grid gap-2">
              <Label htmlFor="snippet-body">Code</Label>
              <Textarea
                id="snippet-body"
                value={createBody}
                onChange={(event) => setCreateBody(event.target.value)}
                className="min-h-64 font-mono text-[13px] leading-5"
                spellCheck={false}
              />
            </div>
            {createError ? <p className="text-sm text-destructive">{createError}</p> : null}
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setCreateOpen(false)} disabled={creating}>
              Cancel
            </Button>
            <Button onClick={() => void createSnippet()} disabled={creating}>
              {creating ? <Loader2 className="size-4 animate-spin" /> : <Plus className="size-4" />}
              Create snippet
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <ActionConfirmationDialog
        open={removeConfirmKey !== null}
        title="Remove snippet?"
        description={`This permanently deletes ${removeConfirmSnippet?.name ?? 'this snippet'} from your user snippets. This cannot be undone.`}
        confirmLabel="Remove snippet"
        busy={removing}
        onOpenChange={(open) => {
          if (!open) setRemoveConfirmKey(null)
        }}
        onConfirm={() => void confirmRemove()}
      />
    </>
  )
}

function DetailButton({
  label,
  icon,
  onClick,
  busy,
  disabled,
  primary,
  danger,
}: {
  label: string
  icon: React.ReactNode
  onClick: () => void
  busy?: boolean
  disabled?: boolean
  primary?: boolean
  danger?: boolean
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        gap: 6,
        height: 28,
        padding: primary ? '0 13px' : '0 12px',
        borderRadius: 8,
        border: primary
          ? '1px solid color-mix(in srgb, var(--aurora-accent-primary) 55%, var(--aurora-border-strong))'
          : danger
            ? '1px solid color-mix(in srgb, var(--aurora-error) 45%, var(--aurora-border-strong))'
            : '1px solid color-mix(in srgb, var(--aurora-border-default) 70%, var(--aurora-page-bg))',
        background: primary
          ? 'color-mix(in srgb, var(--aurora-accent-primary) 9%, var(--aurora-panel-strong))'
          : danger
            ? 'color-mix(in srgb, var(--aurora-error) 9%, var(--aurora-panel-strong))'
            : 'var(--aurora-control-surface)',
        color: primary
          ? 'var(--aurora-accent-strong)'
          : danger
            ? 'var(--aurora-error)'
            : 'var(--aurora-text-muted)',
        fontFamily: 'inherit',
        fontSize: 11.5,
        fontWeight: 650,
        cursor: disabled ? 'progress' : 'pointer',
        opacity: disabled && !busy ? 0.55 : 1,
      }}
    >
      {busy ? <Loader2 className="animate-spin" size={11} /> : icon}
      {label}
    </button>
  )
}
