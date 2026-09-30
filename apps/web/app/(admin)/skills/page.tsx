'use client'

import { Suspense } from 'react'
import { useSearchParams } from 'next/navigation'

import { AppHeader } from '@/components/app-header'
import { Skeleton } from '@/components/ui/skeleton'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { SkillLibraryPageContent } from '@/components/skills/skill-library-page'
import { SkillsPageContent } from '@/components/skills/skills-page-content'
import { ArtifactControlPlane } from '@/components/skills/artifact-control-plane'

function SkillsPageQuery() {
  const searchParams = useSearchParams()
  const upstream = searchParams.get('upstream')?.trim() || undefined
  const requestedView = searchParams.get('view')
  const initialTab = requestedView === 'control-plane' ? 'control-plane' : upstream ? 'upstreams' : 'library'
  return (
    <>
      <AppHeader breadcrumbs={[{ label: 'Skills' }]} />
      <div className="flex-1 px-6 pb-10 pt-4">
        <Tabs defaultValue={initialTab}>
          <TabsList aria-label="Skills views">
            <TabsTrigger value="library">Library</TabsTrigger>
            <TabsTrigger value="upstreams">Upstreams</TabsTrigger>
            <TabsTrigger value="control-plane">Control plane</TabsTrigger>
          </TabsList>
          <TabsContent value="library" className="mt-4"><SkillLibraryPageContent /></TabsContent>
          <TabsContent value="upstreams" className="mt-4"><SkillsPageContent upstream={upstream} embedded /></TabsContent>
          <TabsContent value="control-plane" className="mt-4"><ArtifactControlPlane /></TabsContent>
        </Tabs>
      </div>
    </>
  )
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
