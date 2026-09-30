'use client'

import { useEffect, useState } from 'react'
import Link from 'next/link'
import { AlertTriangle, X } from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { useGatewayNotifications } from '@/lib/notification-acknowledgements'

const STORAGE_KEY = 'dashboard:warnings-dismissed'

/**
 * Server-warning banner. Dismissable, and the dismissal persists (keyed by the
 * warning `signature`) so it stays hidden across reloads — but re-surfaces when
 * the underlying warnings change.
 */
export function WarningsBanner({
  count,
  signature,
  notificationKeys = [],
  href = '/gateways',
}: {
  count: number
  signature: string
  notificationKeys?: string[]
  href?: string
}) {
  const [dismissed, setDismissed] = useState(false)
  const { isDismissed } = useGatewayNotifications()

  useEffect(() => {
    try {
      setDismissed(localStorage.getItem(STORAGE_KEY) === signature)
    } catch {
      setDismissed(false)
    }
  }, [signature])

  const acknowledgedInBell = notificationKeys.length > 0 && notificationKeys.every(isDismissed)
  if (count <= 0 || dismissed || acknowledgedInBell) return null

  const dismiss = () => {
    try {
      localStorage.setItem(STORAGE_KEY, signature)
    } catch {
      toast.warning('Warning dismissal will only last for this session.', {
        description: 'Browser storage is unavailable, so this dashboard preference cannot be persisted.',
      })
    }
    setDismissed(true)
  }

  return (
    <div className="rounded-aurora-3 border border-aurora-warn/30 bg-aurora-warn/8 p-4">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center">
        <div className="flex min-w-0 items-start gap-3">
          <div className="rounded-aurora-1 border border-aurora-warn/30 bg-aurora-warn/12 p-2">
            <AlertTriangle className="size-5 text-aurora-warn" />
          </div>
          <div className="min-w-0 flex-1">
            <p className="font-semibold text-aurora-warn">
              {count} warning{count !== 1 ? 's' : ''} across servers
            </p>
            <p className="text-sm text-aurora-text-muted">
              Review unhealthy or overexposed servers before publishing more downstream tools.
            </p>
          </div>
        </div>
        <div className="flex items-center gap-1.5 sm:ml-auto">
          <Button variant="outline" size="sm" className="flex-1 sm:flex-none" asChild>
            <Link href={href}>View servers</Link>
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className="shrink-0 text-aurora-text-muted hover:text-aurora-text-primary"
            onClick={dismiss}
            aria-label="Dismiss warning"
          >
            <X className="size-4" />
          </Button>
        </div>
      </div>
    </div>
  )
}
