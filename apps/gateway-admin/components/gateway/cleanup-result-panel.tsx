'use client'

import { ShieldAlert, Wrench, X } from 'lucide-react'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Button } from '@/components/ui/button'
import type { Gateway, GatewayCleanupResult } from '@/lib/types/gateway'
import {
  DETAIL_STAT_GRID_STYLE,
  DetailInset,
  DetailMicroLabel,
  DetailStatCard,
} from './gateway-detail-chrome'

interface CleanupResultPanelProps {
  result: { gateway: Gateway; result: GatewayCleanupResult } | null
  onClose: () => void
}

export function CleanupResultPanel({ result, onClose }: CleanupResultPanelProps) {
  if (!result) return null

  const { gateway, result: cleanup } = result
  const isPreview = cleanup.dry_run
  const totalMatched =
    cleanup.gateway_matched +
    cleanup.local_matched +
    cleanup.aggressive_matched
  const totalKilled =
    cleanup.gateway_killed + cleanup.local_killed + cleanup.aggressive_killed
  const totalPrimary = isPreview ? totalMatched : totalKilled
  const laneLabel = isPreview ? 'matches' : 'terminated'
  const laneVerb = isPreview ? 'matched' : 'terminated'
  const renderMatches = (
    title: string,
    matches: GatewayCleanupResult['gateway_matches'],
  ) => {
    if (matches.length === 0) return null
    return (
      <div className="space-y-2">
        <DetailMicroLabel>{title}</DetailMicroLabel>
        <div className="space-y-2">
          {matches.map((match) => (
            <DetailInset key={match.pattern}>
              <div className="flex items-center justify-between gap-3">
                <code className="text-xs">{match.pattern}</code>
                <span className="text-xs font-medium tabular-nums">
                  {match.pids.length} pid{match.pids.length === 1 ? '' : 's'}
                </span>
              </div>
              <p className="mt-2 text-xs text-aurora-text-muted break-all">
                {match.pids.join(', ')}
              </p>
            </DetailInset>
          ))}
        </div>
      </div>
    )
  }

  return (
    <Sheet open={!!result} onOpenChange={(open) => !open && onClose()}>
      <SheetContent className="sm:max-w-md">
        <SheetHeader>
          <SheetTitle>Cleanup Results</SheetTitle>
          <SheetDescription>
            Runtime cleanup results for {gateway.name}
          </SheetDescription>
        </SheetHeader>

        {/* px-4 matches SheetHeader's p-4 — without it the body ran flush to
            the sheet edges and the stat grid clipped on the right. */}
        <div className="mt-2 space-y-6 px-4">
          <div
            style={{ borderRadius: 9 }}
            className={`flex items-start gap-4 border p-4 ${
              cleanup.aggressive
                ? 'border-aurora-warn/20 bg-aurora-warn/5'
                : 'border-aurora-success/20 bg-aurora-success/5'
            }`}
          >
            {cleanup.aggressive ? (
              <ShieldAlert className="size-5 text-aurora-warn mt-0.5" />
            ) : (
              <Wrench className="size-5 text-aurora-success mt-0.5" />
            )}
            <div className="flex-1">
              <p
                className={`font-medium ${
                  cleanup.aggressive ? 'text-aurora-warn' : 'text-aurora-success'
                }`}
              >
                {isPreview
                  ? cleanup.aggressive
                    ? 'Aggressive cleanup preview'
                    : 'Runtime cleanup preview'
                  : cleanup.aggressive
                    ? 'Aggressive cleanup completed'
                    : 'Runtime cleanup completed'}
              </p>
              <p className="text-sm text-aurora-text-muted mt-0.5">
                {totalPrimary} process{totalPrimary === 1 ? '' : 'es'} {laneVerb}.
              </p>
              <p className="mt-2 text-xs text-aurora-text-muted">
                Server-side tracked matches, local leaked session workers, and the aggressive fallback lane are reported separately below.
              </p>
            </div>
          </div>

          <div className="space-y-3">
            <DetailMicroLabel>Cleanup breakdown</DetailMicroLabel>
            {/* Mock stat-card chrome — see gateway-detail-chrome.tsx. */}
            <div style={DETAIL_STAT_GRID_STYLE}>
              <DetailStatCard
                label="Server runtime"
                value={isPreview ? cleanup.gateway_matched : cleanup.gateway_killed}
                sub={laneLabel}
              />
              <DetailStatCard
                label="Local client/session"
                value={isPreview ? cleanup.local_matched : cleanup.local_killed}
                sub={laneLabel}
              />
              {cleanup.aggressive && (
                <DetailStatCard
                  label="Aggressive fallback"
                  value={isPreview ? cleanup.aggressive_matched : cleanup.aggressive_killed}
                  sub={laneLabel}
                />
              )}
            </div>
          </div>

          <div className="space-y-4">
            {renderMatches('Server runtime patterns', cleanup.gateway_matches)}
            {renderMatches('Local client/session patterns', cleanup.local_matches)}
            {cleanup.aggressive && renderMatches('Aggressive fallback patterns', cleanup.aggressive_matches)}
          </div>
        </div>

        <div className="mt-8 px-4 pb-4">
          <Button variant="outline" onClick={onClose} className="w-full">
            <X className="size-4 mr-2" />
            Close
          </Button>
        </div>
      </SheetContent>
    </Sheet>
  )
}
