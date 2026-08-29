'use client'

import { useEffect, useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { upstreamOauthApi } from '@/lib/api/upstream-oauth-client'
import { useUpstreamOauthStatus } from '@/lib/hooks/use-upstream-oauth'
import { openIsolatedOauthPopup } from '@/lib/oauth-popup'

interface UpstreamOauthCardProps {
  name: string
}

export function UpstreamOauthCard({ name }: UpstreamOauthCardProps) {
  const [connecting, setConnecting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const { data: status, mutate } = useUpstreamOauthStatus(name, {
    pollWhilePending: connecting,
  })

  useEffect(() => {
    if (connecting && status?.authenticated) {
      setConnecting(false)
    }
  }, [connecting, status?.authenticated])

  async function handleConnect() {
    setError(null)
    const popup = openIsolatedOauthPopup()
    if (!popup) {
      setError('Popup blocked — please allow popups for this site and try again')
      return
    }
    setConnecting(true)
    try {
      const { authorization_url } = await upstreamOauthApi.start(name)
      if (popup.closed) {
        setConnecting(false)
        setError('Authorization tab was closed — try connecting again')
      } else {
        popup.location.href = authorization_url
      }
    } catch (err: unknown) {
      popup.close()
      setConnecting(false)
      setError(err instanceof Error ? err.message : 'Failed to start authorization')
    }
  }

  const isSharedGoogle = status?.credential_source === 'google_provider'
  const hasSharedCredential = isSharedGoogle
    && status?.google_credential_broker?.provider_generation !== undefined
  const missingScopes = status?.google_credential_broker?.missing_scopes ?? []

  async function handleDisconnect() {
    setError(null)
    try {
      if (isSharedGoogle) {
        const confirmed = window.confirm(
          'Revoke shared Google access? This disconnects every Google MCP server and dependent Labby session using this account.',
        )
        if (!confirmed) return
        await upstreamOauthApi.revokeGoogle(name)
      } else {
        await upstreamOauthApi.clear(name)
      }
      await mutate()
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : 'Failed to disconnect')
    }
  }

  const badge = (() => {
    if (!status) return <Badge variant="outline">Loading…</Badge>
    switch (status.state) {
      case 'connected':
        return <Badge variant="outline" className="border-aurora-success/40 text-aurora-success">Connected</Badge>
      case 'expiring':
        return <Badge variant="outline" className="border-aurora-warn/40 text-aurora-warn">Expiring</Badge>
      case 'expired':
        return <Badge variant="outline" className="border-aurora-error/40 text-aurora-error">Expired</Badge>
      case 'refresh_failed':
        return <Badge variant="outline" className="border-aurora-error/40 text-aurora-error">Refresh failed</Badge>
      case 'scope_upgrade_required':
        return <Badge variant="outline" className="border-aurora-warn/40 text-aurora-warn">Scope upgrade required</Badge>
      case 'discovery_failed':
        return <Badge variant="outline" className="border-aurora-error/40 text-aurora-error">Unavailable</Badge>
      case 'disconnected':
        return <Badge variant="outline" className="text-aurora-text-muted">Disconnected</Badge>
      default:
        if (status.authenticated && status.expires_within_5m)
          return <Badge variant="outline" className="border-aurora-warn/40 text-aurora-warn">Expiring</Badge>
        if (status.authenticated)
          return <Badge variant="outline" className="border-aurora-success/40 text-aurora-success">Connected</Badge>
        return <Badge variant="outline" className="text-aurora-text-muted">Disconnected</Badge>
    }
  })()

  const statusDetail = (() => {
    if (!status) return null
    if (status.state === 'scope_upgrade_required') {
      return missingScopes.length > 0
        ? `Grant ${missingScopes.length} missing Google scope${missingScopes.length === 1 ? '' : 's'}`
        : 'Grant the Google scopes required by this server'
    }
    if (status.refresh_error) return status.refresh_error
    if (status.discovery_error) return status.discovery_error
    if (status.refreshed) return 'Token refreshed'
    if (status.state === 'expired') return 'Access token expired'
    if (status.state === 'connected' && status.discovered_tool_count !== undefined)
      return `${status.exposed_tool_count ?? status.discovered_tool_count} of ${status.discovered_tool_count} tools exposed`
    return null
  })()

  return (
    <Card>
      <CardHeader className="pb-2">
        <div className="flex items-center justify-between gap-2">
          <div className="flex items-center gap-2">
            <CardTitle className="text-sm font-medium">{name}</CardTitle>
            {isSharedGoogle && <Badge variant="secondary">Shared Google</Badge>}
          </div>
          {badge}
        </div>
      </CardHeader>
      <CardContent className="flex flex-col gap-2 pt-0">
        {error && <p className="text-xs text-destructive">{error}</p>}
        {statusDetail && <p className="text-xs text-aurora-text-muted">{statusDetail}</p>}
        {missingScopes.length > 0 && (
          <details className="text-xs text-aurora-text-muted">
            <summary className="cursor-pointer">Missing Google scopes</summary>
            <ul className="mt-1 list-disc space-y-1 pl-5">
              {missingScopes.map((scope) => <li key={scope} className="break-all">{scope}</li>)}
            </ul>
          </details>
        )}
        <div className="flex items-center gap-2">
          {status?.authenticated ? (
            <Button
              variant={isSharedGoogle ? "destructive" : "outline"}
              size="sm"
              onClick={handleDisconnect}
            >
              {isSharedGoogle ? 'Revoke shared access' : 'Disconnect'}
            </Button>
          ) : (
            <Button size="sm" onClick={handleConnect} disabled={connecting}>
              {connecting
                ? 'Waiting…'
                : status?.state === 'scope_upgrade_required'
                  ? 'Grant scopes'
                  : 'Connect'}
            </Button>
          )}
          {!status?.authenticated && hasSharedCredential && (
            <Button variant="destructive" size="sm" onClick={handleDisconnect}>
              Revoke shared access
            </Button>
          )}
          {connecting && (
            <p className="text-xs text-aurora-text-muted">
              Complete authorization in the new tab
            </p>
          )}
        </div>
      </CardContent>
    </Card>
  )
}
