'use client'

import type { ReactNode } from 'react'

import { useCapabilities } from '@/lib/hooks/use-capabilities'
import type { CapabilityKey } from '@/lib/capabilities'
import { ServiceUnavailableNotice } from '@/components/service-unavailable-notice'

/**
 * Renders `children` unless the required capability is confidently absent from
 * the server build, in which case it shows a "not available in this build"
 * notice instead of letting the page fail with an opaque `404`.
 *
 * Fail-open: while the catalog is loading (or on fetch error) the capability is
 * reported available, so the page renders normally and the notice only appears
 * once the catalog confirms the service is gated out.
 */
export function CapabilityGuard({
  need,
  label,
  children,
}: {
  need: CapabilityKey
  label: string
  children: ReactNode
}) {
  const capabilities = useCapabilities()
  if (!capabilities[need]) {
    return <ServiceUnavailableNotice serviceName={label} />
  }
  return <>{children}</>
}
