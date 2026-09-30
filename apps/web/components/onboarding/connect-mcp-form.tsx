'use client'

import { useRef, useState } from 'react'
import { Loader2, Plug, ShieldCheck } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { gatewayApi } from '@/lib/api/gateway-client'
import type { CreateGatewayInput, Gateway } from '@/lib/types/gateway'
import { mcpConnectionVerified } from './readiness-model'

/** Shared first-use composition of the existing protected gateway operations. */
export function ConnectMcpForm({ onConnected, initialName = '' }: {
  onConnected?: (gateway: Gateway | null) => void
  initialName?: string
}) {
  const [name, setName] = useState(initialName)
  const [target, setTarget] = useState('')
  const [transport, setTransport] = useState<'http' | 'stdio'>('http')
  const [args, setArgs] = useState('[]')
  const [auth, setAuth] = useState<'none' | 'bearer' | 'oauth'>('none')
  const [secret, setSecret] = useState('')
  const [approved, setApproved] = useState(false)
  const [saved, setSaved] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const pending = useRef(false)

  const connect = async () => {
    if (pending.current || !approved) return
    pending.current = true
    setBusy(true); setError(null); setMessage(null); onConnected?.(null)
    let serverName = saved
    try {
      if (!serverName) {
        if (!/^[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}$/.test(name.trim())) throw new Error('Use a server name containing letters, numbers, underscores, or hyphens.')
        const config: CreateGatewayInput['config'] = { proxy_resources: true, proxy_prompts: true }
        if (transport === 'http') {
          const url = new URL(target.trim())
          if (!['https:', 'http:'].includes(url.protocol) || url.username || url.password || url.hash || url.search) throw new Error('Use an HTTP(S) endpoint without embedded credentials, a query, or a fragment.')
          if (auth === 'bearer' && !secret.trim()) throw new Error('Enter the server API key, or choose a different authentication method.')
          config.url = url.toString()
          if (auth === 'bearer') {
            if (url.protocol !== 'https:' && !['localhost', '127.0.0.1', '[::1]'].includes(url.hostname)) throw new Error('Use HTTPS when sending a server credential outside loopback.')
            config.bearer_token_value = secret.trim()
          }
          if (auth === 'oauth') config.oauth = { registration_strategy: 'dynamic' }
        } else {
          const parsed: unknown = JSON.parse(args)
          if (!Array.isArray(parsed) || parsed.length > 64 || !parsed.every(value => typeof value === 'string' && value.length <= 4096)) throw new Error('Arguments must be a JSON array of strings, with at most 64 entries.')
          if (!target.trim() || target.length > 2048 || /[\r\n]/.test(target)) throw new Error('Enter the executable name or its absolute path, without shell commands.')
          config.command = target.trim(); config.args = parsed
        }
        const created = await gatewayApi.create({ name: name.trim(), transport, config })
        serverName = created.id
        setSaved(serverName)
        setSecret('')
      }
      const test = await gatewayApi.test(serverName)
      const gateway = await gatewayApi.get(serverName)
      if (!test.success || !mcpConnectionVerified(gateway)) {
        throw new Error(auth === 'oauth'
          ? 'Server saved. Complete authorization on Gateway, then retry this connection check.'
          : 'Server saved, but a healthy connection with exposed tools is not ready yet. Check Gateway diagnostics, then retry.')
      }
      setMessage(`${gateway.name}: connected, ${gateway.status.exposed_tool_count} tools available. A tools-list check is not a tool execution test.`)
      onConnected?.(gateway)
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : 'The server could not be connected.')
    } finally {
      pending.current = false; setBusy(false); setSecret('')
    }
  }

  return <form className="space-y-4" onSubmit={event => { event.preventDefault(); void connect() }} aria-label="Connect MCP server">
    <p className="text-sm text-aurora-text-muted">Use the publisher’s documented endpoint or executable. Importing an artifact into Library does not start a server.</p>
    <fieldset disabled={busy || Boolean(saved)} className="space-y-4 disabled:opacity-70">
      <div className="grid gap-4 sm:grid-cols-2">
        <div className="space-y-2"><Label htmlFor="first-mcp-name">Server name</Label><Input id="first-mcp-name" value={name} onChange={event => setName(event.target.value)} placeholder="my-first-server" autoComplete="off" required /></div>
        <div className="space-y-2"><Label htmlFor="first-mcp-transport">Connection</Label><select id="first-mcp-transport" className="h-10 w-full rounded-md border border-aurora-border-default bg-aurora-control-surface px-3 text-sm" value={transport} onChange={event => { setTransport(event.target.value as 'http' | 'stdio'); setApproved(false) }}><option value="http">Remote endpoint</option><option value="stdio">Local executable (advanced)</option></select></div>
      </div>
      <div className="space-y-2"><Label htmlFor="first-mcp-target">{transport === 'http' ? 'MCP endpoint' : 'Executable on the gateway host'}</Label><Input id="first-mcp-target" value={target} onChange={event => { setTarget(event.target.value); setApproved(false) }} placeholder={transport === 'http' ? 'https://server.example/mcp' : 'uvx'} autoComplete="off" required /></div>
      {transport === 'stdio' ? <div className="space-y-2"><Label htmlFor="first-mcp-args">Arguments (JSON array)</Label><Input id="first-mcp-args" value={args} onChange={event => { setArgs(event.target.value); setApproved(false) }} className="font-mono" /><p className="text-xs text-aurora-text-muted">This runs on the Labby host, not in your browser. A package runner may download code. Environment variables and advanced permissions remain on Gateway.</p></div> : <div className="grid gap-4 sm:grid-cols-2"><div className="space-y-2"><Label htmlFor="first-mcp-auth">Authentication</Label><select id="first-mcp-auth" className="h-10 w-full rounded-md border border-aurora-border-default bg-aurora-control-surface px-3 text-sm" value={auth} onChange={event => { setAuth(event.target.value as typeof auth); setSecret('') }}><option value="none">No credential required</option><option value="bearer">API key / bearer token</option><option value="oauth">Sign in with OAuth</option></select></div>{auth === 'bearer' ? <div className="space-y-2"><Label htmlFor="first-mcp-key">Server API key</Label><Input id="first-mcp-key" type="password" value={secret} onChange={event => setSecret(event.target.value)} autoComplete="new-password" /></div> : null}</div>}
    </fieldset>
    <label className="flex items-start gap-3 text-sm"><input type="checkbox" className="mt-1 size-4 shrink-0" checked={approved} disabled={busy} onChange={event => setApproved(event.target.checked)} /><span>I trust this server and approve connecting it from the Labby host. For a local executable, this may download and run software.</span></label>
    {error ? <p role="alert" className="text-sm text-aurora-error">{error} <a className="underline" href="/gateway">Open Gateway diagnostics</a>.</p> : null}
    {message ? <p role="status" className="flex items-start gap-2 text-sm text-aurora-success"><ShieldCheck aria-hidden="true" className="size-4 shrink-0" />{message}</p> : null}
    <Button type="submit" disabled={busy || !approved || (!saved && (!name.trim() || !target.trim()))}>{busy ? <Loader2 aria-hidden="true" className="size-4 animate-spin" /> : <Plug aria-hidden="true" className="size-4" />}{busy ? 'Checking server…' : saved ? 'Retry connection check' : 'Connect and check server'}</Button>
  </form>
}
