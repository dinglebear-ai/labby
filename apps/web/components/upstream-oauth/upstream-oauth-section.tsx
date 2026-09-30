'use client'

import { useState } from 'react'
import { Plus } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { AURORA_MEDIUM_PANEL, AURORA_MUTED_LABEL } from '@/components/gateway/gateway-theme'
import { useUpstreamOauthUpstreams } from '@/lib/hooks/use-upstream-oauth'
import { ConnectUpstreamDialog } from './connect-upstream-dialog'
import { UpstreamOauthCard } from './upstream-oauth-card'

export function UpstreamOauthSection() {
  const { data: upstreams, error, isLoading, mutate } = useUpstreamOauthUpstreams()
  const [dialogOpen, setDialogOpen] = useState(false)

  const header = (
    <div className="mb-3 flex items-center justify-between">
      <h2 className="text-lg font-semibold">Upstream Connections</h2>
      <Button size="sm" variant="outline" onClick={() => setDialogOpen(true)}>
        <Plus className="mr-1.5 size-3.5" />
        Connect
      </Button>
    </div>
  )

  return (
    <section className={cn(AURORA_MEDIUM_PANEL, 'space-y-4 p-4')}>
      {header}

      {isLoading && <p className="text-sm text-aurora-text-muted">Loading…</p>}

      {error && (
        error.message.includes('404')
          ? (
            <div className="rounded-aurora-2 border border-aurora-border-strong/50 bg-aurora-control-surface px-4 py-3">
              <p className={AURORA_MUTED_LABEL}>OAuth relay</p>
              <p className="mt-2 text-sm text-aurora-text-muted">
                Upstream OAuth is unavailable in this environment.
              </p>
            </div>
            )
          : (
            <div className="rounded-aurora-2 border border-aurora-error/30 bg-aurora-error/10 px-4 py-3">
              <p className={AURORA_MUTED_LABEL}>OAuth relay</p>
              <p className="mt-2 text-sm text-aurora-error">
                Failed to load upstream list: {error.message}
              </p>
            </div>
            )
      )}

      {!isLoading && !error && upstreams && upstreams.length > 0 && (
        <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {upstreams.map((u) => (
            <UpstreamOauthCard key={u.name} name={u.name} />
          ))}
        </div>
      )}

      {!isLoading && !error && (!upstreams || upstreams.length === 0) && (
        <div className="rounded-aurora-2 border border-aurora-border-strong/50 bg-aurora-control-surface px-4 py-3 text-sm text-aurora-text-muted">
          No upstream connections configured.
        </div>
      )}

      <ConnectUpstreamDialog
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        onConnected={() => void mutate()}
      />
    </section>
  )
}
