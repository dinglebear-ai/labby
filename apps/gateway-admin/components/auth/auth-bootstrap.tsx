'use client'

import * as React from 'react'
import { SWRConfig } from 'swr'
import { getBrowserSessionContextIdentity } from '../../lib/auth/session-store.ts'

import { LoginScreen } from './login-screen.tsx'
import { OwnerSetupScreen } from './owner-setup-screen.tsx'
import { shouldBypassBrowserSessionAuth } from '../../lib/auth/auth-mode.ts'
import { loadBrowserSession, useBrowserSession } from '../../lib/auth/session.ts'

type AuthBootstrapProps = {
  children: React.ReactNode
}

export function AuthBootstrap({ children }: AuthBootstrapProps) {
  const session = useBrowserSession()
  const bypassBrowserSessionAuth = shouldBypassBrowserSessionAuth()

  React.useEffect(() => {
    if (!bypassBrowserSessionAuth && session.status === 'loading') {
      void loadBrowserSession()
    }
  }, [bypassBrowserSessionAuth, session.status])

  if (bypassBrowserSessionAuth) {
    return <>{children}</>
  }

  if (session.status === 'loading') {
    return (
      <div className="flex min-h-screen items-center justify-center text-sm text-aurora-text-muted">
        Checking session…
      </div>
    )
  }

  const returnTo =
    typeof window === 'undefined'
      ? '/'
      : `${window.location.pathname}${window.location.search}${window.location.hash}`

  if (session.status === 'unauthenticated') {
    return <LoginScreen returnTo={returnTo} />
  }

  if (session.status === 'auth_error') {
    return (
      <LoginScreen
        errorMessage={session.message}
        requestId={session.requestId}
        returnTo={returnTo}
      />
    )
  }

  if (session.authorityState === 'transport' || session.authorityState === 'unprovisioned') {
    return (
      <OwnerSetupScreen
        authorityState={session.authorityState}
        email={session.user.email}
        remediation={session.remediation}
      />
    )
  }

  return (
    <SWRConfig key={getBrowserSessionContextIdentity()} value={{ provider: () => new Map() }}>
      {children}
    </SWRConfig>
  )
}
