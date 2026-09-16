'use client'

import * as React from 'react'
import { AppHeader } from '@/components/app-header'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { ConsoleHero } from '@/components/console/console-hero'
import { loadBrowserSession, useBrowserSession } from '@/lib/auth/session'
import { cn } from '@/lib/utils'

const SETUP_ORIGIN = 'https://labby.dinglebear.ai'
const SETUP_EMBED_URL = '/setup/content?embed=1'
const MIN_FRAME_HEIGHT = 1200
const MAX_FRAME_HEIGHT = 30_000

function clampFrameHeight(value: unknown): number | null {
  if (typeof value !== 'number' || !Number.isFinite(value)) return null
  return Math.min(MAX_FRAME_HEIGHT, Math.max(MIN_FRAME_HEIGHT, Math.ceil(value)))
}

export function PublicSetupPage() {
  const session = useBrowserSession()
  const [frameHeight, setFrameHeight] = React.useState(6800)

  React.useEffect(() => {
    if (session.status === 'loading') void loadBrowserSession()
  }, [session.status])

  React.useEffect(() => {
    const onMessage = (event: MessageEvent) => {
      if (event.origin !== SETUP_ORIGIN) return
      if (!event.data || event.data.type !== 'labby:setup-height') return
      const next = clampFrameHeight(event.data.height)
      if (next !== null) setFrameHeight(next)
    }
    window.addEventListener('message', onMessage)
    return () => window.removeEventListener('message', onMessage)
  }, [])

  return (
    <>
      <AppHeader breadcrumbs={[{ label: 'Gateway', href: '/gateways' }, { label: 'Setup' }]} />
      <div className={cn(AURORA_PAGE_FRAME, AURORA_PAGE_SHELL)} style={{ gap: 14 }}>
        <ConsoleHero
          eyebrow="Gateway Setup"
          title="Labby → Codex → ChatGPT"
          description="Pinned Labby installation, Codex over hardened SSH, public HTTPS, Google OAuth, and ChatGPT app setup in one guided flow."
          pulse={{ color: 'var(--aurora-success)', label: 'public onboarding' }}
          stats={[
            { label: 'Labby', value: 'v1.20.1', tone: 'var(--aurora-accent-strong)' },
            { label: 'Primary client', value: 'Codex' },
            { label: 'HTTPS', value: 'Funnel', tone: 'var(--aurora-success)' },
          ]}
        />
        <iframe
          title="Labby setup instructions"
          src={SETUP_EMBED_URL}
          style={{
            display: 'block',
            width: '100%',
            height: frameHeight,
            border: 0,
            background: 'transparent',
          }}
          referrerPolicy="no-referrer"
        />
      </div>
    </>
  )
}
