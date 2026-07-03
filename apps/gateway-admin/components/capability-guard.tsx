'use client'

import type { ReactNode } from 'react'

import { useCapabilities } from '@/lib/hooks/use-capabilities'
import type { CapabilityKey } from '@/lib/capabilities'
import {
  CapabilityPending,
  ServiceUnavailableNotice,
} from '@/components/service-unavailable-notice'

/**
 * Renders `children` only once the catalog confirms the required capability is
 * available. Until the catalog has resolved (`!caps.ready`) it renders a brief
 * loading placeholder INSTEAD OF the children, so the guarded page's `/v1/*`
 * fetches never fire on a gated build before the catalog can swap in the
 * "not available in this build" notice. Once resolved, a confidently-absent
 * capability shows the notice; otherwise the children render.
 *
 * Fail-open: the per-capability boolean stays available whenever the catalog
 * has NOT given a definitive answer — real data received or errored — not on a
 * loading flag (the catalog's `fallbackData: []` makes `isLoading` false on the
 * first render). So a fetch error renders the children normally; only a
 * resolved catalog that omits the backing service shows the notice.
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
  if (!capabilities.ready) {
    return <CapabilityPending />
  }
  if (!capabilities[need]) {
    return <ServiceUnavailableNotice serviceName={label} />
  }
  return <>{children}</>
}
