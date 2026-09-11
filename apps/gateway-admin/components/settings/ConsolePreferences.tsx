'use client'

import { useTheme } from 'next-themes'
import { SettingsCard, SettingsRow, settingsSegmentStyle } from './SettingsChrome'

export type ConsoleTheme = 'dark' | 'light' | 'system'
const CONSOLE_THEMES: ReadonlyArray<[ConsoleTheme, string]> = [['dark', 'Dark'], ['light', 'Light'], ['system', 'System']]

export function isConsoleTheme(value: unknown): value is ConsoleTheme {
  return value === 'dark' || value === 'light' || value === 'system'
}

export function ConsoleThemeControl({ theme, onTheme }: { theme?: ConsoleTheme; onTheme: (theme: ConsoleTheme) => void }) {
  return <div role="group" aria-label="Console theme" className="flex gap-1">
    {CONSOLE_THEMES.map(([value, label]) =>
      <button key={value} type="button" aria-pressed={theme === value} onClick={() => onTheme(value)} style={settingsSegmentStyle(theme === value)} className="focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary">{label}</button>
    )}
  </div>
}

export function ConsolePreferences() {
  const { theme, setTheme } = useTheme()
  return <SettingsCard title="Console">
    <SettingsRow label="Theme" description="Console appearance. System follows the OS preference." control={<ConsoleThemeControl theme={isConsoleTheme(theme) ? theme : undefined} onTheme={setTheme}/>}/>
  </SettingsCard>
}
