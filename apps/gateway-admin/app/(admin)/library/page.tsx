import { Suspense } from 'react'

import { LibraryPageContent } from '@/components/depot/library-page-content'

export default function Page() {
  return (
    <Suspense fallback={null}>
      <LibraryPageContent />
    </Suspense>
  )
}
