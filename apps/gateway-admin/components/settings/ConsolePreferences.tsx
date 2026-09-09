'use client'

import { useTheme } from 'next-themes'
import { SettingsCard, SettingsRow, settingsSegmentStyle } from './SettingsChrome'

export function ConsoleThemeControl({ theme, onTheme }: { theme?: string; onTheme: (theme: string) => void }) {
  return <div role="group" aria-label="Console theme" className="flex gap-1">
    {['Dark', 'Light', 'System'].map(label => {
      const value = label.toLowerCase()
      return <button key={value} type="button" aria-pressed={theme === value} onClick={() => onTheme(value)} style={settingsSegmentStyle(theme === value)} className="focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary">{label}</button>
    })}
  </div>
}

export function ConsolePreferences() {
  const { theme, setTheme } = useTheme()
  return <SettingsCard title="Console">
    <SettingsRow label="Theme" description="Console appearance. System follows the OS preference." control={<ConsoleThemeControl theme={theme} onTheme={setTheme}/>}/>
  </SettingsCard>
}
