import { Suspense } from 'react'
import { MarketplacePluginPageClient } from './plugin-page-client'
import { CapabilityGuard } from '@/components/capability-guard'

export default function MarketplacePluginPage() {
  return (
    <CapabilityGuard need="marketplace" label="Marketplace">
      <Suspense fallback={null}>
        <MarketplacePluginPageClient />
      </Suspense>
    </CapabilityGuard>
  )
}
