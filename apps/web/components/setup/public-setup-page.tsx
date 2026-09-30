'use client'

import * as React from 'react'
import { AppHeader } from '@/components/app-header'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { loadBrowserSession, useBrowserSession } from '@/lib/auth/session'
import { cn } from '@/lib/utils'

const SETUP_ORIGIN = 'https://labby.dinglebear.ai'
const SETUP_EMBED_URL = '/setup/content?embed=1'
const MIN_FRAME_HEIGHT = 800
const MAX_FRAME_HEIGHT = 24_000

function clampFrameHeight(value: unknown): number | null {
  if (typeof value !== 'number' || !Number.isFinite(value)) return null
  return Math.min(MAX_FRAME_HEIGHT, Math.max(MIN_FRAME_HEIGHT, Math.ceil(value)))
}

export function PublicSetupPage() {
  const session = useBrowserSession()
  const [frameHeight, setFrameHeight] = React.useState(4600)

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
      <div className={cn(AURORA_PAGE_FRAME, AURORA_PAGE_SHELL)} style={{ gap: 8 }}>
        <div
          data-setup-compact-header="1"
          style={{
            minHeight: 42,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'space-between',
            gap: 12,
            padding: '2px 2px 8px',
            borderBottom: '1px solid color-mix(in srgb, var(--aurora-border-default) 62%, transparent)',
            flexWrap: 'wrap',
          }}
        >
          <div style={{ minWidth: 0 }}>
            <div style={{ display: 'flex', alignItems: 'baseline', gap: 9, flexWrap: 'wrap' }}>
              <span style={{ fontSize: 9.5, fontWeight: 750, letterSpacing: '.15em', textTransform: 'uppercase', color: 'var(--aurora-accent-strong)' }}>
                Gateway setup
              </span>
              <h1 style={{ margin: 0, fontFamily: 'var(--font-display)', fontSize: 18, lineHeight: 1.1, fontWeight: 800, color: 'var(--aurora-text-primary)' }}>
                Labby → Codex → ChatGPT
              </h1>
            </div>
            <p style={{ margin: '3px 0 0', fontSize: 11.5, lineHeight: 1.35, color: 'var(--aurora-text-muted)' }}>
              Pinned Labby, hardened SSH, public HTTPS, Google OAuth, and ChatGPT in one compact flow.
            </p>
          </div>
          <div style={{ display: 'flex', alignItems: 'center', gap: 5, flexWrap: 'wrap', fontSize: 9.5, fontWeight: 700 }}>
            {[
              ['v1.20.1', 'var(--aurora-accent-strong)'],
              ['Codex', 'var(--aurora-text-primary)'],
              ['Funnel', 'var(--aurora-success)'],
            ].map(([label, color]) => (
              <span key={label} style={{ height: 22, display: 'inline-flex', alignItems: 'center', padding: '0 8px', borderRadius: 999, border: '1px solid color-mix(in srgb, var(--aurora-border-default) 75%, transparent)', background: 'var(--gw0-0_30)', color }}>
                {label}
              </span>
            ))}
          </div>
        </div>
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
