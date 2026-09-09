'use client'

import { ConsolePreferences } from '@/components/settings/ConsolePreferences'
import { SettingsOverview } from '@/components/settings/SettingsOverview'

export default function CorePage(): React.ReactElement {
  return (
    <SettingsOverview console={<ConsolePreferences />} />
  )
}
