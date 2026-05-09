'use client'

import { useEffect, useState } from 'react'
import { Loader2 } from 'lucide-react'

import { Badge } from '@/components/ui/badge'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { setupApi, type SettingsState } from '@/lib/api/setup-client'

const SURFACE_ROWS = [
  { key: 'mcp.transport', label: 'MCP transport', source: 'mcp.transport / LAB_MCP_TRANSPORT' },
  { key: 'mcp.http', label: 'HTTP bind', source: 'mcp.host + mcp.port / LAB_MCP_HTTP_*' },
  { key: 'web.auth', label: 'Web UI auth', source: 'web.auth_disabled / LAB_WEB_UI_AUTH_DISABLED' },
  { key: 'oauth.public_url', label: 'OAuth public URL', source: 'auth.public_url / LAB_AUTH_PUBLIC_URL' },
]

export default function SurfacesPage(): React.ReactElement {
  const [settings, setSettings] = useState<SettingsState | undefined>()
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | undefined>()

  useEffect(() => {
    const controller = new AbortController()
    setupApi
      .settingsState(controller.signal)
      .then((next) => {
        if (!controller.signal.aborted) setSettings(next)
      })
      .catch((err) => {
        if (!controller.signal.aborted) setError(err instanceof Error ? err.message : 'load failed')
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false)
      })
    return () => controller.abort()
  }, [])

  return (
    <>
      <h1 className="sr-only">Surface settings</h1>
      <Card>
        <CardHeader>
          <CardTitle>Surfaces</CardTitle>
          <CardDescription>
            Runtime surface settings resolved from config and environment.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          {loading ? (
            <div className="flex items-center gap-2 text-sm text-muted-foreground">
              <Loader2 className="h-4 w-4 animate-spin" /> loading settings
            </div>
          ) : null}
          {error ? <p className="text-sm text-destructive">{error}</p> : null}
          {settings ? (
            <p className="text-xs text-muted-foreground">Config path: {settings.config_path}</p>
          ) : null}
          <ul className="divide-y rounded-md border">
            {SURFACE_ROWS.map((row) => (
              <li key={row.key} className="flex items-center justify-between gap-3 p-3">
                <div>
                  <p className="text-sm font-medium">{row.label}</p>
                  <p className="text-xs text-muted-foreground">{row.source}</p>
                </div>
                <Badge variant="secondary">read-only</Badge>
              </li>
            ))}
          </ul>
        </CardContent>
      </Card>
    </>
  )
}
