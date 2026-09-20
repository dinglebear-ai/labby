'use client'

import { Suspense } from 'react'

import { AppHeader } from '@/components/app-header'
import { DocsPageContent } from '@/components/docs/docs-page-content'
import { Skeleton } from '@/components/ui/skeleton'

export default function DocsPage() {
  return (
    <Suspense
      fallback={
        <>
          <AppHeader breadcrumbs={[{ label: 'Documentation' }]} />
          <div className="grid gap-4 lg:grid-cols-[280px_minmax(0,1fr)]">
            <Skeleton className="h-[520px] w-full rounded-aurora-2" />
            <Skeleton className="h-[620px] w-full rounded-aurora-2" />
          </div>
        </>
      }
    >
      <DocsPageContent />
    </Suspense>
  )
}
