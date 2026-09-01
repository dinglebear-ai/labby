import { Suspense } from 'react'

import { AppHeader } from '@/components/app-header'
import { GatewayListContent } from '@/components/gateway/gateway-list-content'
import { Skeleton } from '@/components/ui/skeleton'

export default function GatewaysPage() {
  return (
    <Suspense
      fallback={
        <>
          <AppHeader breadcrumbs={[{ label: 'Gateways' }]} />
          <div className="flex-1 p-6">
            <Skeleton className="h-[420px] w-full rounded-lg" />
          </div>
        </>
      }
    >
      <GatewayListContent />
    </Suspense>
  )
}
