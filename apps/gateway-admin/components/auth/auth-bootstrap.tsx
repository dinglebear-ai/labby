'use client'

import * as React from 'react'
import { SWRConfig } from 'swr'
import { getBrowserSessionContextIdentity, ownerBootstrapOffered } from '../../lib/auth/session-store.ts'

import { LoginScreen } from './login-screen.tsx'
import { OrganizationProfileImportScreen, TeamEnrollmentCompleteScreen } from './organization-profile-handoff-screen.tsx'
import { OwnerSetupScreen, TeamInvitationScreen } from './owner-setup-screen.tsx'
import { shouldBypassBrowserSessionAuth } from '../../lib/auth/auth-mode.ts'
import { loadBrowserSession, useBrowserSession } from '../../lib/auth/session.ts'
import {
  ORGANIZATION_PROFILE_IMPORT_SESSION_KEY,
  ORGANIZATION_PROFILE_OFFER_SESSION_KEY,
  organizationProfileOfferFromHash,
  parseOrganizationProfileOfferJson,
} from '../../lib/auth/organization-profile-handoff.ts'
import {
  normalizeTeamInvitationToken,
  TEAM_INVITATION_TOKEN_SESSION_KEY,
  teamInvitationTokenFromHash,
} from '../../lib/auth/team-invitation.ts'

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

  React.useEffect(() => {
    if (bypassBrowserSessionAuth || typeof window === 'undefined') return
    const invitationToken = teamInvitationTokenFromHash(window.location.hash)
    const organizationImport = organizationProfileOfferFromHash(window.location.hash)
    if (invitationToken) {
      window.sessionStorage.setItem(TEAM_INVITATION_TOKEN_SESSION_KEY, invitationToken)
    }
    if (organizationImport) {
      window.sessionStorage.setItem(
        ORGANIZATION_PROFILE_IMPORT_SESSION_KEY,
        JSON.stringify(organizationImport),
      )
    }
    if (!invitationToken && !organizationImport) return
    // Never let onboarding material flow into OAuth return_to, reverse-proxy
    // logs, or referrer headers. Same-tab session storage carries it across sign-in.
    window.history.replaceState(null, '', window.location.pathname + window.location.search)
  }, [bypassBrowserSessionAuth])

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
      : `${window.location.pathname}${window.location.search}`
  const pendingInvitationToken =
    typeof window === 'undefined'
      ? undefined
      : teamInvitationTokenFromHash(window.location.hash) ??
        normalizeTeamInvitationToken(window.sessionStorage.getItem(TEAM_INVITATION_TOKEN_SESSION_KEY))
  const pendingOrganizationImport =
    typeof window === 'undefined'
      ? undefined
      : organizationProfileOfferFromHash(window.location.hash) ??
        parseOrganizationProfileOfferJson(window.sessionStorage.getItem(ORGANIZATION_PROFILE_IMPORT_SESSION_KEY))
  const teamOrganizationOffer =
    typeof window === 'undefined'
      ? undefined
      : parseOrganizationProfileOfferJson(window.sessionStorage.getItem(ORGANIZATION_PROFILE_OFFER_SESSION_KEY))

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

  if (session.authorityState === 'ready' && pendingInvitationToken) {
    return <TeamInvitationScreen email={session.user.email} />
  }

  if (session.authorityState === 'ready' && pendingOrganizationImport) {
    return <OrganizationProfileImportScreen offer={pendingOrganizationImport} />
  }

  if (session.authorityState === 'ready' && teamOrganizationOffer) {
    return <TeamEnrollmentCompleteScreen offer={teamOrganizationOffer} />
  }

  if (session.authorityState === 'transport' || session.authorityState === 'unprovisioned') {
    return (
      <OwnerSetupScreen
        authorityState={session.authorityState}
        bootstrapAvailable={ownerBootstrapOffered(session)}
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
