'use client'

import * as React from 'react'

import { ConsoleShellProvider } from '@/components/console/console-shell-context'
import { ConsoleSidebar } from '@/components/console/console-sidebar'
import { ConsoleTopbar } from '@/components/console/console-topbar'

/**
 * The Gateway Console frame: a full-viewport flex row of sidebar + main column,
 * where the main column owns the page's aurora wash, the single topbar, and the
 * only vertical scroll container. Screens render inside the scroll body and are
 * centred on the mock's 1740px measure.
 */
export function ConsoleShell({ children }: { children: React.ReactNode }) {
  return (
    <ConsoleShellProvider>
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
        <ConsoleSidebar />

        <div
          style={{
            flex: 1,
            minWidth: 0,
            display: 'flex',
            flexDirection: 'column',
            background:
              'radial-gradient(circle at 12% -4%, rgba(41,182,246,0.09), transparent 30%), radial-gradient(circle at 88% -6%, rgba(103,203,250,0.06), transparent 24%), var(--aurora-page-bg)',
          }}
        >
          <ConsoleTopbar />

          <main style={{ flex: 1, overflowY: 'auto', overflowX: 'hidden', minHeight: 0 }}>
            <div
              data-main-scroll="1"
              style={{
                maxWidth: 1740,
                margin: '0 auto',
                padding: '20px 24px 40px',
                display: 'flex',
                flexDirection: 'column',
                gap: 16,
              }}
            >
              {children}
            </div>
          </main>
        </div>
      </div>
    </ConsoleShellProvider>
  )
}
