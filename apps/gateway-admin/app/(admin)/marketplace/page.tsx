import { MarketplaceListContent } from '@/components/marketplace/marketplace-list-content'
import { CapabilityGuard } from '@/components/capability-guard'

export const metadata = { title: 'Marketplace — Labby' }

export default function MarketplacePage() {
  return (
    <CapabilityGuard need="marketplace" label="Marketplace">
      <MarketplaceListContent />
    </CapabilityGuard>
  )
}
