import type { ReactNode } from 'react'
import { Settings as SettingsIcon } from 'lucide-react'

import { AppHeader } from '@/components/app-header'
import { SettingsRail } from '@/components/settings/SettingsRail'
import { SettingsPageHeader } from '@/components/settings/SettingsChrome'
import { DraftStaleBanner } from '@/components/settings/DraftStaleBanner'

/** ConsoleShell owns the page measure; Settings supplies the section grid. */
export default function SettingsLayout({
  children,
}: {
  children: ReactNode
}): React.ReactElement {
  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        gap: 24,
        width: '100%',
        minWidth: 0,
      }}
    >
      <AppHeader icon={<SettingsIcon size={18} strokeWidth={1.7} />} breadcrumbs={[{ label: 'Settings' }]} />
      <SettingsPageHeader
        title="Settings"
        description="Gateway behavior and console preferences."
      />
      <div className="grid min-w-0 gap-6 lg:grid-cols-[208px_minmax(0,1fr)]">
      <SettingsRail />
      <main style={{ display: 'flex', flexDirection: 'column', gap: 24, minWidth: 0 }}>
        <DraftStaleBanner />
        {children}
      </main>
      </div>
    </div>
  )
}
