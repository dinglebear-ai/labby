// Dynamic route for one service. Required for `output: 'export'`:
// `generateStaticParams` returns every slug at build time.

import { Suspense } from 'react'
import ServicePage from './service-client'
import { SERVICE_SLUGS } from '@/lib/setup/buildServiceSlugs'

export function generateStaticParams(): Array<{ service: string }> {
  return SERVICE_SLUGS.map((slug) => ({ service: slug }))
}

export default async function Page({
  params,
}: {
  params: Promise<{ service: string }>
}): Promise<React.ReactElement> {
  const { service } = await params
  return (
    <Suspense fallback={null}>
      <ServicePage service={service} />
    </Suspense>
  )
}
