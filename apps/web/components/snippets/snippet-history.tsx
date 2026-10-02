'use client'

import { useEffect, useRef, useState, useSyncExternalStore } from 'react'
import { Button } from '@/components/ui/button'
import { snippetsApi } from '@/lib/api/snippets-client'
import { getBrowserSessionEpoch, subscribeToBrowserSession } from '@/lib/auth/session-store'
import type { SnippetExecutionReceipt } from '@/lib/types/snippets'

export function SnippetHistory({ name, revision, onReplay }: { name: string; revision: number; onReplay?: (receipt:SnippetExecutionReceipt)=>void }) {
  const epoch = useSyncExternalStore(subscribeToBrowserSession, getBrowserSessionEpoch, () => 0)
  return <HistoryScope key={`${name}:${epoch}:${revision}`} name={name} revision={revision} epoch={epoch} onReplay={onReplay} />
}

function HistoryScope({ name, revision, epoch, onReplay }: { name: string; revision: number; epoch: number; onReplay?: (receipt:SnippetExecutionReceipt)=>void }) {
  const [receipts, setReceipts] = useState<SnippetExecutionReceipt[]>([])
  const [cursor, setCursor] = useState<string | null>(null)
  const [detail, setDetail] = useState<SnippetExecutionReceipt | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [disabled, setDisabled] = useState(false)
  const [loading, setLoading] = useState(true)
  const [refresh, setRefresh] = useState(0)
  const [page, setPage] = useState<string | undefined>()
  useEffect(() => {
    const controller = new AbortController()
    setLoading(true); setReceipts([]); setDetail(null); setSelectedId(undefined); setError(null); setDisabled(false); setCursor(null)
    snippetsApi.history({ name, limit: 20, ...(page ? { cursor: page } : {}) }, controller.signal)
      .then((response) => { if (!controller.signal.aborted) { setReceipts(response.receipts ?? []); setCursor(response.next_cursor ?? null); setDisabled(response.receipt_status === 'disabled') } })
      .catch((error) => { if (!controller.signal.aborted) setError(error instanceof Error ? error.message : 'Unable to load run history.') })
      .finally(() => { if (!controller.signal.aborted) setLoading(false) })
    return () => controller.abort()
  }, [name, revision, epoch, refresh, page])
  const downloadControllers = useRef(new Set<AbortController>())
  useEffect(() => {
    const controllers = downloadControllers.current
    return () => { for (const controller of controllers) controller.abort(); controllers.clear() }
  }, [])
  const [downloading, setDownloading] = useState<string>()
  const download = async (receipt: SnippetExecutionReceipt, path: string) => {
    const controller = new AbortController()
    downloadControllers.current.add(controller)
    setDownloading(path); setError(null)
    try {
      const artifact = await snippetsApi.artifact(receipt.execution_id, path, controller.signal)
      if (controller.signal.aborted || epoch !== getBrowserSessionEpoch()) return
      if (artifact.bytes > 8 * 1024 * 1024 || artifact.content_base64.length > 12 * 1024 * 1024) throw new Error('Artifact exceeds the 8 MiB download limit.')
      const decoded = atob(artifact.content_base64)
      if (decoded.length !== artifact.bytes) throw new Error('Artifact download was incomplete.')
      const bytes = Uint8Array.from(decoded, (char) => char.charCodeAt(0))
      const url = URL.createObjectURL(new Blob([bytes], { type: artifact.content_type }))
      const anchor = document.createElement('a')
      anchor.href = url; anchor.download = artifact.path.split('/').pop() || 'artifact'
      anchor.click(); URL.revokeObjectURL(url)
    } catch (error) { if (!controller.signal.aborted && epoch === getBrowserSessionEpoch()) setError(error instanceof Error ? error.message : 'Artifact unavailable or no longer retained.') }
    finally { downloadControllers.current.delete(controller); if (!controller.signal.aborted) setDownloading(undefined) }
  }
  const [selectedId, setSelectedId] = useState<string>()
  useEffect(() => {
    if (!selectedId) return
    const controller = new AbortController()
    setDetail(null)
    snippetsApi.receipt(selectedId, controller.signal)
      .then((receipt) => { if (!controller.signal.aborted) setDetail(receipt) })
      .catch((error) => { if (!controller.signal.aborted) setError(error instanceof Error ? error.message : 'Receipt unavailable.') })
    return () => controller.abort()
  }, [selectedId, name, epoch])
  return <section className="grid min-w-0 gap-3" aria-label={`Run history for ${name}`}>
    <div className="flex flex-wrap items-center justify-between gap-2"><h3 className="text-xs font-bold uppercase tracking-wider text-aurora-text-muted">Run history</h3><Button type="button" size="sm" variant="outline" disabled={loading} onClick={() => { setPage(undefined); setSelectedId(undefined); setRefresh((value) => value + 1) }}>Refresh history</Button></div>
    {error ? <p role="alert" className="text-xs text-aurora-error">{error}</p> : null}
    {loading ? <p role="status" className="text-xs text-aurora-text-muted">Loading history…</p> : disabled ? <p className="text-xs text-aurora-text-muted">Persistent receipts are disabled on this gateway.</p> : receipts.length === 0 ? <p className="text-xs text-aurora-text-muted">No retained runs for this snippet in the current workspace.</p> : <div className="grid gap-2">{receipts.map((receipt) => <Button key={receipt.execution_id} type="button" variant="outline" className="h-auto justify-start whitespace-normal text-left" aria-pressed={selectedId === receipt.execution_id} onClick={() => setSelectedId(receipt.execution_id)}><span className="grid gap-1"><span>{new Date(receipt.created_at_ms).toLocaleString()} · {receipt.status} · {receipt.elapsed_ms} ms</span><span className="break-all text-xs text-aurora-text-muted">{receipt.tool_calls} calls · {receipt.execution_id}</span></span></Button>)}</div>}
    {cursor ? <Button type="button" size="sm" variant="outline" disabled={loading} onClick={() => { setSelectedId(undefined); setPage(cursor) }}>Next history page</Button> : null}
    {detail ? <div className="grid gap-3 rounded-aurora-2 border border-aurora-border-subtle p-3 text-xs">
      {onReplay ? <Button size="sm" variant="outline" onClick={()=>onReplay(detail)}>Preview replay</Button> : null}
      <p className="break-all">{detail.execution_id} · {detail.status} · {detail.elapsed_ms} ms{detail.error_kind ? ` · ${detail.error_kind}` : ''}</p>
      <dl className="grid min-w-0 gap-1 text-aurora-text-muted"><dt>Runtime</dt><dd>{detail.runtime_version} · {detail.surface}</dd><dt>Snippet digest</dt><dd className="break-all">{detail.snippet_digest}</dd><dt>Input digest</dt><dd className="break-all">{detail.input_digest}</dd></dl>
      <div><h4 className="mb-2 font-semibold">Tool calls</h4>{detail.calls.map((call, index) => <p key={index} className={call.ok ? 'text-aurora-text-muted' : 'text-aurora-error'}>{index + 1}. {call.tool} · {call.ok ? 'succeeded' : `failed${call.error_kind ? ` (${call.error_kind})` : ''}`} · {call.elapsed_ms} ms</p>)}{detail.omitted_calls > 0 ? <p>{detail.omitted_calls} additional calls omitted.</p> : null}</div>
      {detail.artifacts.length > 0 ? <div><h4 className="mb-2 font-semibold">Artifact references</h4>{detail.artifacts.map((artifact, index) => <div key={index} className="flex flex-wrap items-center gap-2"><p className="min-w-0 break-all text-aurora-text-muted">{artifact.path} · {artifact.bytes} bytes · {artifact.content_type}</p><Button type="button" size="sm" variant="outline" disabled={downloading !== undefined} onClick={() => void download(detail, artifact.path)}>{downloading === artifact.path ? 'Downloading…' : 'Download artifact'}</Button></div>)}<p className="mt-2 text-aurora-text-muted">References do not guarantee that artifact bytes are still retained.</p></div> : null}
    </div> : null}
  </section>
}
