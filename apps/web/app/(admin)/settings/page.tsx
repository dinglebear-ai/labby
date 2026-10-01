'use client'

import Link from 'next/link'

import { SettingsCard, SettingsRow } from '@/components/settings/SettingsChrome'
import { FirstUseReadiness } from '@/components/settings/FirstUseReadiness'

export default function SettingsIndex(): React.ReactElement {
  return <>
    <FirstUseReadiness />
    <SettingsCard title="Gateway configuration" description="Change how this installation listens, logs, and exposes its public endpoints. Each field explains where the value takes effect and whether Labby must restart.">
      <SettingsRow label="Gateway basics" description="Server address and port, logs, workspace, and CLI defaults." control={<Link href="/settings/core/" className="text-xs font-semibold text-aurora-accent-strong hover:underline">Open</Link>} />
      <SettingsRow label="Client access" description="Public app and MCP addresses, accepted hosts, and browser origins." control={<Link href="/settings/surfaces/" className="text-xs font-semibold text-aurora-accent-strong hover:underline">Open</Link>} />
      <SettingsRow label="Diagnostics" description="Inspect effective configuration and run gateway checks." control={<Link href="/settings/doctor/" className="text-xs font-semibold text-aurora-accent-strong hover:underline">Open</Link>} />
    </SettingsCard>
  </>
}
