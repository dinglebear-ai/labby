'use client'
import { useEffect, useState } from 'react'
import { Sheet, SheetContent, SheetHeader, SheetTitle, SheetDescription } from '@/components/ui/sheet'
import { describeCodeModeTool, type ToolDescription } from '@/lib/api/tool-browser-client'
import { subscribeToBrowserSession } from '@/lib/auth/session-store'
import { getErrorMessage } from '@/lib/utils'
export function GatewayToolInspector({ target, descriptor, onClose }: { target: string | null; descriptor?: { name: string; description?: string; exposed: boolean; uri?: string; arguments?: Array<{ name: string; description?: string; required?: boolean }> } | null; onClose: () => void }) {
  const [detail, setDetail] = useState<ToolDescription>()
  const [error, setError] = useState<string>()
  const [attempt, setAttempt] = useState(0)
  useEffect(() => {
    setDetail(undefined); setError(undefined)
    if (!target || descriptor) return
    const controller = new AbortController()
    const unsubscribe = subscribeToBrowserSession(() => { controller.abort(); setDetail(undefined); setError('Session changed. Close and reopen to inspect this tool.') })
    void describeCodeModeTool(target, controller.signal).then((value) => { if (!controller.signal.aborted) setDetail(value) }, (cause) => { if (!controller.signal.aborted) setError(getErrorMessage(cause, 'Tool inspection unavailable')) })
    return () => { controller.abort(); unsubscribe() }
  }, [target, attempt, descriptor])
  return <Sheet open={target !== null} onOpenChange={(open) => { if (!open) onClose() }}><SheetContent className="overflow-y-auto sm:max-w-xl"><SheetHeader><SheetTitle>{descriptor ? 'Inspect catalog entry' : 'Inspect tool'}</SheetTitle><SheetDescription className="break-all font-mono">{target}</SheetDescription></SheetHeader><div className="space-y-4 p-4">{descriptor ? <><h3 className="font-mono text-sm">{descriptor.name}</h3><p className="text-sm">{descriptor.description || 'No description returned.'}</p><p className="text-xs text-aurora-text-muted">{descriptor.exposed ? 'Exposed through this server' : 'Hidden from clients'}</p>{descriptor.uri ? <p className="break-all font-mono text-xs">{descriptor.uri}</p> : null}{descriptor.arguments?.length ? <dl className="space-y-2 text-xs">{descriptor.arguments.map((argument) => <div key={argument.name}><dt className="font-mono">{argument.name}{argument.required ? ' · required' : ' · optional'}</dt><dd className="text-aurora-text-muted">{argument.description}</dd></div>)}</dl> : null}</> : error ? <div role="alert" className="text-sm text-aurora-error">{error}<button type="button" onClick={() => setAttempt((value) => value + 1)} className="ml-3 underline">Retry</button></div> : detail ? <><p className="text-sm">{detail.description}</p><dl className="space-y-2 text-xs"><dt>Helper</dt><dd className="break-all font-mono">{detail.helper}</dd><dt>Safety</dt><dd>{detail.safety?.destructive ? 'Destructive' : detail.safety?.read_only ? 'Read only' : 'Unspecified'}</dd></dl><h3 className="text-xs font-semibold">Parameters · TypeScript</h3>{detail.typescript ? <pre className="overflow-auto rounded-lg bg-aurora-page-bg p-3 text-xs"><code>{detail.typescript}</code></pre> : <p className="text-xs text-aurora-text-muted">{detail.typescript_omitted === 'size_limit' ? 'Declaration exceeds the response limit.' : 'No declaration returned.'}</p>}<p className="text-xs text-aurora-text-muted">Execute through an MCP client. This console provides inspection without a browser execution endpoint.</p></> : <p className="text-sm text-aurora-text-muted">Loading live tool definition…</p>}</div></SheetContent></Sheet>
}
