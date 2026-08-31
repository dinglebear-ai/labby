'use client'

import { Suspense } from 'react'
import { DepotPageContent } from '@/components/depot/depot-page-content'

export default function DepotPage() {
  return <Suspense fallback={<div className="p-6">Loading Depot…</div>}><DepotPageContent /></Suspense>
}
