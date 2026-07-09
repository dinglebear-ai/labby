'use client'

// Settings index — redirects to the Core panel. The fleet-posture
// dashboard that previously lived here was moved to /settings/doctor/
// (lab-bg3e.5; the Doctor panel renders the same stat-card content
// alongside doctor.audit.full results).

import { useEffect } from 'react'
import { useRouter } from 'next/navigation'
import { Loader2 } from 'lucide-react'

export default function SettingsIndex(): React.ReactElement {
  const router = useRouter()
  useEffect(() => {
    router.replace('/settings/core/')
  }, [router])
  return (
    <div className="flex items-center gap-2 text-sm text-aurora-text-muted">
      <Loader2 className="h-4 w-4 animate-spin" />
      Loading settings…
    </div>
  )
}
