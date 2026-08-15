import type { ReactNode } from 'react'

import { AppHeader } from '@/components/app-header'
import { SettingsRail } from '@/components/settings/SettingsRail'
import { SettingsPageHeader, SETTINGS_MEASURE } from '@/components/settings/SettingsChrome'
import { DraftStaleBanner } from '@/components/settings/DraftStaleBanner'

/**
 * Settings shell, measured off the mock's `section[data-screen-label="Settings"]`:
 * a flex column with a 14px gap capped at 760px, opening with a 24px display
 * title over a muted lede. `ConsoleShell` already supplies the scroll body's
 * padding, so this adds none of its own.
 */
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
        gap: 14,
        width: '100%',
        minWidth: 0,
        maxWidth: SETTINGS_MEASURE,
      }}
    >
      <AppHeader breadcrumbs={[{ label: 'Settings' }]} />
      <SettingsPageHeader
        title="Settings"
        description="Gateway behavior, service credentials, and console preferences."
      />
      <SettingsRail />
      <main style={{ display: 'flex', flexDirection: 'column', gap: 14, minWidth: 0 }}>
        <DraftStaleBanner />
        {children}
      </main>
    </div>
  )
}
