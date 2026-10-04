'use client'
import { useEffect, useRef, useState } from 'react'
import { initialOperationForm, operationParams, type OperationFormState } from './operation-form'
import { Button } from '@/components/ui/button'
import { listFirstUseTools, verifyFirstUseTool, type FirstUseTool } from '@/lib/depot/mcp-activation'

export function DiscoverMcpToolVerification({ server, endpoint }: { server: string; endpoint: string }) {
  const [tools, setTools] = useState<FirstUseTool[]>([])
  const [selected, setSelected] = useState('')
  const [inputs, setInputs] = useState<OperationFormState>({})
  const [approved, setApproved] = useState(false)
  const [pending, setPending] = useState(false)
  const [loading, setLoading] = useState(true)
  const [result, setResult] = useState<string>()
  const [reload, setReload] = useState(0)
  const busy = useRef(false)
  useEffect(() => {
    const controller = new AbortController()
    setLoading(true); setTools([]); setSelected(''); setApproved(false); setResult(undefined)
    void listFirstUseTools(server, endpoint, controller.signal).then(value => {
      if (controller.signal.aborted) return
      setTools(value); setSelected(value[0]?.name ?? ''); setInputs(initialOperationForm(value[0]?.inputSchema?.properties ?? {}))
    }).catch(error => { if (!controller.signal.aborted) setResult(error instanceof Error ? error.message : 'Verification tools are unavailable.') }).finally(() => { if (!controller.signal.aborted) setLoading(false) })
    return () => controller.abort()
  }, [server, endpoint, reload])
  async function verify() {
    if (busy.current || !approved || !tools.some(tool => tool.name === selected)) return
    busy.current = true; setPending(true); setResult(undefined)
    try { await verifyFirstUseTool(server, endpoint, selected, tool!.reviewFingerprint, operationParams(tool?.inputSchema?.properties ?? {}, tool?.inputSchema?.required ?? [], inputs)); setResult(`Tool call verified: ${selected}. The server recorded this first-use result.`) }
    catch (error) { setResult(error instanceof Error ? error.message : 'The tool check failed. Readiness remains incomplete.') }
    finally { busy.current = false; setPending(false); setApproved(false) }
  }
  const tool = tools.find(tool => tool.name === selected)
  let inputError: string | undefined
  try { operationParams(tool?.inputSchema?.properties ?? {}, tool?.inputSchema?.required ?? [], inputs) } catch (error) { inputError = error instanceof Error ? error.message : 'Complete the tool inputs.' }
  return <section aria-label="Verify first MCP tool" className="space-y-3 border-t border-aurora-border-default pt-3">
    <h4 className="font-semibold">Verify a tool call</h4>
    <p className="text-sm text-aurora-text-muted">Choose an exposed tool that declares read-only behavior. Fill in its supported inputs. These declarations come from the server; review the tool and destination before authorizing one call. The check has a 10-second deadline and a 16 KiB response limit.</p>
    {loading ? <p role="status">Checking supported tools…</p> : tools.length ? <>
      <label className="block text-sm">Tool<select aria-label="Verification tool" className="mt-1 block w-full rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface p-2" disabled={pending} value={selected} onChange={event => { setSelected(event.target.value); setInputs(initialOperationForm(tools.find(tool => tool.name === event.target.value)?.inputSchema?.properties ?? {})); setApproved(false); setResult(undefined) }}>{tools.map(tool => <option key={tool.name} value={tool.name}>{tool.name}</option>)}</select></label>
      <p className="text-sm text-aurora-text-muted">{tool?.description || 'This tool has no description. Review it in Gateway before authorizing a call.'}</p>
      {Object.entries(tool?.inputSchema?.properties ?? {}).map(([name, property]) => <label key={name} className="block space-y-1 text-sm">
        <span>{name}{tool?.inputSchema?.required?.includes(name) ? ' (required)' : ' (optional)'}</span>
        {property.description ? <span className="block text-aurora-text-muted">{property.description}</span> : null}
        {property.enum || property.type === 'boolean' ? <select aria-label={name} disabled={pending} className="block w-full rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface p-2" value={String(inputs[name] ?? '')} onChange={event => { setInputs(value => ({ ...value, [name]: property.type === 'boolean' ? (event.target.value === '' ? undefined : event.target.value === 'true') : event.target.value })); setApproved(false) }}>
          <option value="">Select a value</option>{(property.enum ?? [true, false]).map(value => <option key={String(value)} value={String(value)}>{String(value)}</option>)}
        </select> : <input aria-label={name} disabled={pending} className="block w-full rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface p-2" type={property.type === 'number' || property.type === 'integer' ? 'number' : 'text'} min={property.minimum} max={property.maximum} step={property.type === 'integer' ? 1 : 'any'} minLength={property.minLength} maxLength={property.maxLength ?? 8192} value={String(inputs[name] ?? '')} onChange={event => { setInputs(value => ({ ...value, [name]: event.target.value })); setApproved(false) }}/>}
      </label>)}
      <label className="flex gap-2 text-sm"><input type="checkbox" checked={approved} disabled={pending} onChange={event => setApproved(event.target.checked)}/>Allow one call to {selected} at {endpoint}, with the inputs shown above.</label>
      {inputError ? <p role="status" className="text-sm text-aurora-text-muted">{inputError}</p> : null}
      <Button disabled={pending || !approved || Boolean(inputError)} onClick={() => void verify()}>{pending ? 'Calling tool…' : 'Run selected tool check'}</Button>
    </> : <p className="text-sm text-aurora-text-muted">No supported read-only tool is exposed. This first-use check remains incomplete; review tools and input requirements in Gateway.</p>}
    {result ? <p role="status" className="text-sm">{result}</p> : null}
    <Button variant="outline" disabled={pending || loading} onClick={() => setReload(value => value + 1)}>Reload supported tools</Button>
  </section>
}
