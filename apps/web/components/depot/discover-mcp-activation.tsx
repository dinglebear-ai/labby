'use client'

import { useRef, useState } from 'react'
import Link from 'next/link'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { DiscoverMcpToolVerification } from './discover-mcp-tool-verification'
import { gatewayApi } from '@/lib/api/gateway-client'
import { gatewayDetailHref } from '@/lib/api/gateway-config'
import type { FederatedArtifact } from '@/lib/api/depot-client'
import { activateCatalogMcp, catalogGatewayInput, supportedMcpConnection } from '@/lib/depot/mcp-activation'

export function DiscoverMcpActivation({ artifact }: { artifact: FederatedArtifact }) {
  const connection = supportedMcpConnection(artifact)
  const [name, setName] = useState((artifact.name ?? 'catalog-server').replace(/[^a-zA-Z0-9_-]/g, '-').slice(0, 64))
  const [token, setToken] = useState('')
  const [approved, setApproved] = useState(false)
  const [savedId, setSavedId] = useState<string>()
  const [message, setMessage] = useState<string>()
  const [pending, setPending] = useState(false)
  const [connected, setConnected] = useState(false)
  const busy = useRef(false)
  const validName = /^[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}$/.test(name)
  const validToken = Boolean(token.trim()) && !/[\r\n\0]/.test(token)
  if (!connection) return artifact.kind === 'mcp-server' || artifact.kind === 'mcp' ? <p className="text-sm text-aurora-text-muted">This revision does not provide a supported MCP endpoint. Add to Library saves its files; connect it from Gateway after reviewing its installation requirements.</p> : null

  async function activate() {
    if (busy.current || !approved) return
    busy.current = true
    setPending(true)
    setMessage(undefined)
    try {
      const result = savedId ? await gatewayApi.test(savedId) : await activateCatalogMcp(catalogGatewayInput(artifact, name, token), gateway => { setSavedId(gateway.id); setToken('') })
      setConnected(result.success)
      setMessage(result.success
        ? `Connection verified. ${result.discovered_tools ?? 0} tools discovered. Choose a tool below to verify its first call.`
        : `Server saved; connection needs attention: ${result.detail ?? result.error ?? result.message}`)
    } catch (error) {
      setConnected(false)
      setMessage(error instanceof Error ? error.message : 'Connection could not be verified. Retry from Gateway if the server was saved.')
    } finally { busy.current = false; setPending(false) }
  }

  return <section aria-label="Add MCP server" className="space-y-3 rounded-aurora-2 border border-aurora-border-default bg-aurora-panel-medium p-[var(--space-4)]">
    <h3 className="font-semibold">Add MCP server</h3>
    <p className="text-sm text-aurora-text-muted">Labby’s server will connect to this HTTPS endpoint and expose its tools. Review the destination and access before connecting.</p>
    <code className="block break-all text-xs">{connection.url}</code>
    <p className="text-xs text-aurora-text-muted">Revision: {connection.revisionId} · Authentication: {connection.authentication === 'bearer' ? 'bearer token' : 'none'}</p>
    <label className="block text-sm">Server name<Input aria-label="MCP server name" value={name} disabled={pending || Boolean(savedId)} onChange={event => setName(event.target.value)} maxLength={64} aria-invalid={!validName}/><span className="text-xs text-aurora-text-muted">Use 1–64 letters, numbers, hyphens, or underscores. Existing server names cannot be replaced here.</span></label>
    {connection.authentication === 'bearer' && !savedId ? <label className="block text-sm">Bearer token<Input type="password" autoComplete="off" aria-label="MCP bearer token" value={token} disabled={pending} onChange={event => setToken(event.target.value)}/><span className="text-xs text-aurora-text-muted">Stored by the Labby server through its protected credential writer. The value is never returned.</span></label> : null}
    <label className="flex items-start gap-2 text-sm"><input type="checkbox" checked={approved} disabled={pending} onChange={event => setApproved(event.target.checked)}/>Allow Labby to connect to this endpoint and make its tools available.</label>
    <Button disabled={pending || !approved || !validName || (connection.authentication === 'bearer' && !savedId && !validToken)} onClick={() => void activate()}>{pending ? 'Checking connection…' : savedId ? 'Retry connection check' : 'Add MCP server and check connection'}</Button>
    {message ? <p role="status" className="text-sm">{message}</p> : null}
    {savedId && connected ? <DiscoverMcpToolVerification key={savedId} server={savedId} endpoint={connection.url} /> : null}
    {savedId ? <Link className="text-sm underline" href={gatewayDetailHref(savedId)}>Open server to review capabilities and connection settings</Link> : null}
  </section>
}
