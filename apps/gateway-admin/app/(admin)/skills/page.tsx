'use client'

import { Suspense } from 'react'
import { useSearchParams } from 'next/navigation'

import { AppHeader } from '@/components/app-header'
import { Skeleton } from '@/components/ui/skeleton'
import { SkillsPageContent } from '@/components/skills/skills-page-content'

function SkillsPageQuery() {
  const searchParams = useSearchParams()
  const upstream = searchParams.get('upstream')?.trim() || undefined
  return <SkillsPageContent upstream={upstream} />
}

export default function SkillsPage() {
  return (
    <Suspense
      fallback={
        <>
          <AppHeader breadcrumbs={[{ label: 'Skills' }]} />
          <div className="flex-1 p-6">
            <Skeleton className="h-[420px] w-full rounded-lg" />
          </div>
        </>
      }
    >
      <SkillsPageQuery />
    </Suspense>
  )
}
