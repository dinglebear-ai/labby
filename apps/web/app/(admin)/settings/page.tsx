'use client'

import Link from 'next/link'
import { ArrowRight, Network, Settings2, Stethoscope } from 'lucide-react'
import { SettingsCard } from '@/components/settings/SettingsChrome'
import { FirstUseReadiness } from '@/components/settings/FirstUseReadiness'

const TASKS = [
  { title: 'Gateway basics', description: 'Set the server address, port, logging, workspace, and CLI defaults.', href: '/settings/core/', icon: Settings2 },
  { title: 'Client access', description: 'Configure public app and MCP addresses, accepted hosts, and browser origins.', href: '/settings/surfaces/', icon: Network },
  { title: 'Diagnostics', description: 'Inspect the configuration Labby is using and run gateway checks.', href: '/settings/doctor/', icon: Stethoscope },
]

export default function SettingsIndex(): React.ReactElement {
  return <>
    <FirstUseReadiness />
    <SettingsCard title="Gateway configuration" description="Manage this installation. Each setting explains what it controls and whether a restart is needed.">
      <div className="grid gap-3 p-4 sm:grid-cols-3 sm:p-5">
        {TASKS.map(({ title, description, href, icon: Icon }) => <Link key={href} href={href} className="flex min-w-0 flex-col rounded-lg border border-aurora-border-default/60 bg-aurora-control-surface p-4 transition-colors hover:border-aurora-border-strong hover:bg-aurora-hover-bg focus-visible:outline-2 focus-visible:outline-aurora-accent-primary">
          <Icon className="mb-4 size-5 text-aurora-accent-strong" aria-hidden="true" />
          <h3 className="text-sm font-semibold text-aurora-text-primary">{title}</h3>
          <p className="mt-2 flex-1 text-sm leading-relaxed text-aurora-text-muted">{description}</p>
          <span className="mt-4 inline-flex items-center gap-1.5 text-sm font-medium text-aurora-accent-strong">Open settings<ArrowRight className="size-3.5" aria-hidden="true" /></span>
        </Link>)}
      </div>
    </SettingsCard>
  </>
}
