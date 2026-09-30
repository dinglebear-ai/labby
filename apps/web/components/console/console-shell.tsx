'use client'

import * as React from 'react'
import dynamic from 'next/dynamic'

import { ConsoleShellProvider, useConsoleShell } from '@/components/console/console-shell-context'
import { ConsoleSidebar } from '@/components/console/console-sidebar'
import { ConsoleTopbar } from '@/components/console/console-topbar'
import { CapabilityHealthBanner } from '@/components/console/capability-health-banner'

const ConsoleGlobalTools = dynamic(
  () => import('@/components/console/console-global-tools').then((module) => module.ConsoleGlobalTools),
  { ssr: false },
)

/**
 * The Gateway Console frame: a full-viewport flex row of sidebar + main column,
 * where the main column owns the page's aurora wash, the single topbar, and the
 * only vertical scroll container. Screens render inside the scroll body and are
 * centred on the mock's 1740px measure.
 */
export function ConsoleShell({
  children,
  publicSetup = false,
}: {
  children: React.ReactNode
  publicSetup?: boolean
}) {
  return (
    <ConsoleShellProvider>
      <ConsoleShellFrame publicSetup={publicSetup}>{children}</ConsoleShellFrame>
    </ConsoleShellProvider>
  )
}

function ConsoleShellFrame({
  children,
  publicSetup = false,
}: {
  children: React.ReactNode
  publicSetup?: boolean
}) {
  const { phoenixDocked } = useConsoleShell()

  return (
      <div
        className="console-root"
        data-screen-label="Gateway Console"
        style={{
          display: 'flex',
          height: '100vh',
          overflow: 'hidden',
          background: 'var(--aurora-page-bg)',
          color: 'var(--aurora-text-primary)',
          fontFamily: 'var(--font-sans)',
          fontSize: 14,
        }}
      >
        <ConsoleSidebar publicSetup={publicSetup} />

        <div
          data-console-main-column="1"
          data-phoenix-docked={phoenixDocked ? 'right' : 'float'}
          style={{
            flex: 1,
            minWidth: 0,
            display: 'flex',
            flexDirection: 'column',
            background:
              'radial-gradient(circle at 12% -4%, rgba(41,182,246,0.09), transparent 30%), radial-gradient(circle at 88% -6%, rgba(103,203,250,0.06), transparent 24%), var(--aurora-page-bg)',
          }}
        >
          <ConsoleTopbar publicSetup={publicSetup} />

          <main style={{ flex: 1, overflowY: 'auto', overflowX: 'hidden', minHeight: 0 }}>
            <div
              data-main-scroll="1"
              style={{
                width: '100%',
                maxWidth: 'none',
                margin: 0,
                padding: '12px 22px 40px 11px',
                display: 'flex',
                flexDirection: 'column',
                gap: 16,
              }}
            >
              <CapabilityHealthBanner />
              {children}
            </div>
          </main>
          <ConsoleGlobalTools />
        </div>
        {phoenixDocked ? (
          <div
            aria-hidden="true"
            data-phoenix-dock-spacer="right"
            className="hidden w-[min(420px,36vw)] shrink-0 sm:block"
          />
        ) : null}
      </div>
  )
}
