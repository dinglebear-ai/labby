'use client'

import * as React from 'react'

import { LabbyIcon } from '../labby-icon.tsx'
import {
  AURORA_CONTROL_SURFACE,
  AURORA_DISPLAY_1,
  AURORA_MUTED_LABEL,
  AURORA_PAGE_SHELL,
  AURORA_STRONG_PANEL,
} from '../aurora/tokens.ts'
import { accessApi, AccessApiError } from '../../lib/api/access-client.ts'
import { bootstrapOwner, describeOwnerBootstrapError } from '../../lib/auth/owner-bootstrap.ts'
import { LogoutRevocationError, loadBrowserSession, logoutBrowserSession } from '../../lib/auth/session.ts'
import { ORGANIZATION_PROFILE_OFFER_SESSION_KEY } from '../../lib/auth/organization-profile-handoff.ts'
import { requestTeamAdmission } from '../../lib/auth/team-admission.ts'
import {
  normalizeTeamInvitationToken,
  TEAM_INVITATION_TOKEN_SESSION_KEY,
  teamInvitationTokenFromHash,
} from '../../lib/auth/team-invitation.ts'
import { cn } from '../../lib/utils.ts'

type OwnerSetupScreenProps = {
  authorityState: 'transport' | 'unprovisioned'
  /** Server-declared: no owner exists yet and this caller may claim ownership. */
  bootstrapAvailable: boolean
  remediation?: string
  email?: string | null
}

// Inlined for the same reason as login-screen.tsx: auth-bootstrap.test.tsx
// renders this under node:test, which cannot resolve the `@/` alias the
// Button and Input primitives rely on. Every class is an Aurora token.
const PRIMARY_BUTTON =
  'mt-6 inline-flex h-10 w-full items-center justify-center gap-2 whitespace-nowrap ' +
  'rounded-md bg-primary px-6 text-sm font-medium text-primary-foreground ' +
  'transition-all hover:bg-aurora-accent-strong/95 disabled:pointer-events-none disabled:opacity-60 ' +
  'focus-visible:ring-aurora-accent-primary/34 focus-visible:ring-[3px] outline-none'
const FIELD_INPUT = cn(
  AURORA_CONTROL_SURFACE,
  'mt-1.5 h-10 w-full border px-3 text-sm text-aurora-text-primary outline-none',
  'focus-visible:ring-aurora-accent-primary/34 focus-visible:ring-[3px]',
)

const BOOTSTRAP_REMEDIATION =
  'Durable access authority is not initialized yet. Complete owner bootstrap to finish setup.'
const NO_ACCESS_REMEDIATION: Record<OwnerSetupScreenProps['authorityState'], string> = {
  transport: 'This Labby has not been set up yet. Only its configured owner account can finish setup.',
  unprovisioned: 'You are signed in. Use your team invitation to finish joining this Labby.',
}


/**
 * Shown while the server reports a signed-in session without durable access
 * authority. Only when the server declares `owner_bootstrap_available` does
 * it render the form for `POST /v1/access/bootstrap-owner`; every other
 * caller sees a no-access notice. The server still enforces every gate.
 */
export function OwnerSetupScreen({ authorityState, bootstrapAvailable, remediation, email }: OwnerSetupScreenProps) {
  const heading = bootstrapAvailable
    ? 'Finish setting up Labby'
    : authorityState === 'unprovisioned'
      ? 'Join your team'
      : 'No access yet'
  // The transport remediation from the server describes bootstrap; only an
  // eligible caller should see it. Unprovisioned callers get the actionable
  // invitation path instead of an opaque authorization-state message.
  const message = bootstrapAvailable
    ? remediation || BOOTSTRAP_REMEDIATION
    : authorityState === 'unprovisioned'
      ? NO_ACCESS_REMEDIATION.unprovisioned
      : NO_ACCESS_REMEDIATION.transport

  // Give the server's team admission policy one `/v1` request to act on; a
  // qualifying identity reloads as `ready` and this screen unmounts.
  const admissionRequested = React.useRef(false)
  React.useEffect(() => {
    if (authorityState !== 'unprovisioned' || bootstrapAvailable || admissionRequested.current) return
    admissionRequested.current = true
    void requestTeamAdmission()
  }, [authorityState, bootstrapAvailable])

  return (
    <div className={cn(AURORA_PAGE_SHELL, 'flex min-h-screen items-center justify-center px-6')}>
      <div className={cn(AURORA_STRONG_PANEL, 'w-full max-w-md p-8')}>
        <div className="mb-6 flex items-center gap-3">
          <LabbyIcon size={40} />
          <span className="text-xl font-bold text-aurora-text-primary">Labby</span>
        </div>
        <p className={AURORA_MUTED_LABEL}>{bootstrapAvailable ? 'Owner setup' : 'Access'}</p>
        <h1 className={cn(AURORA_DISPLAY_1, 'mt-3 text-aurora-text-primary')}>{heading}</h1>
        <p className="mt-3 text-sm leading-[1.55] text-aurora-text-muted">{message}</p>
        {email ? (
          <p className="mt-2 text-sm text-aurora-text-muted">
            Signed in as <span className="text-aurora-text-primary">{email}</span>
          </p>
        ) : null}
        {bootstrapAvailable ? (
          <OwnerBootstrapForm />
        ) : authorityState === 'unprovisioned' ? (
          <TeamInvitationForm />
        ) : (
          <SignOutButton />
        )}
      </div>
    </div>
  )
}

function OwnerBootstrapForm() {
  const [organizationName, setOrganizationName] = React.useState('Local')
  const [projectName, setProjectName] = React.useState('Default')
  const [customizeNames, setCustomizeNames] = React.useState(false)
  const [pending, setPending] = React.useState(false)
  const [error, setError] = React.useState<string | null>(null)

  async function handleSubmit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setPending(true)
    setError(null)
    try {
      // On success the session reloads as `ready` and this screen unmounts.
      await bootstrapOwner({ organizationName, projectName })
    } catch (caught) {
      setError(describeOwnerBootstrapError(caught))
    } finally {
      setPending(false)
    }
  }

  return (
    <form className="mt-6" onSubmit={handleSubmit}>
      <p className="text-sm leading-[1.55] text-aurora-text-muted">
        Labby can create a ready-to-use local workspace for you. You can rename or reorganize it later without losing any capabilities.
      </p>
      <button
        className="mt-4 text-sm font-medium text-aurora-accent-primary underline-offset-4 hover:underline"
        onClick={() => setCustomizeNames((value) => !value)}
        type="button"
      >
        {customizeNames ? 'Use simple defaults' : 'Customize organization and project names'}
      </button>
      {customizeNames ? (
        <div className="mt-2 rounded-aurora-2 border border-aurora-border/70 p-4">
          <label className="block text-sm font-medium text-aurora-text-primary" htmlFor="owner-setup-organization">
            Organization name
          </label>
          <input
            autoComplete="off"
            className={FIELD_INPUT}
            id="owner-setup-organization"
            maxLength={128}
            name="organization_name"
            onChange={(event) => setOrganizationName(event.target.value)}
            required
            value={organizationName}
          />
          <label className="mt-4 block text-sm font-medium text-aurora-text-primary" htmlFor="owner-setup-project">
            Project name
          </label>
          <input
            autoComplete="off"
            className={FIELD_INPUT}
            id="owner-setup-project"
            maxLength={128}
            name="project_name"
            onChange={(event) => setProjectName(event.target.value)}
            required
            value={projectName}
          />
          <p className="mt-3 text-xs leading-[1.5] text-aurora-text-muted">
            Advanced naming only changes the labels created during bootstrap. Teams, projects, policies, loadouts, routes, and every other gateway feature remain available afterward.
          </p>
        </div>
      ) : (
        <p className="mt-3 text-xs text-aurora-text-muted">Using Local / Default. No decisions required.</p>
      )}
      {error ? <ErrorNotice message={error} /> : null}
      <button className={PRIMARY_BUTTON} disabled={pending} type="submit">
        {pending ? 'Finishing setup…' : 'Finish setup'}
      </button>
    </form>
  )
}

export function TeamInvitationScreen({ email }: { email?: string | null }) {
  return (
    <div className={cn(AURORA_PAGE_SHELL, 'flex min-h-screen items-center justify-center px-6')}>
      <div className={cn(AURORA_STRONG_PANEL, 'w-full max-w-md p-8')}>
        <div className="mb-6 flex items-center gap-3">
          <LabbyIcon size={40} />
          <span className="text-xl font-bold text-aurora-text-primary">Labby</span>
        </div>
        <p className={AURORA_MUTED_LABEL}>Invitation</p>
        <h1 className={cn(AURORA_DISPLAY_1, 'mt-3 text-aurora-text-primary')}>Join your team</h1>
        <p className="mt-3 text-sm leading-[1.55] text-aurora-text-muted">
          This invitation is matched to your verified sign-in email. Joining adds the team without changing your existing workspaces or capabilities.
        </p>
        {email ? <p className="mt-2 text-sm text-aurora-text-muted">Signed in as <span className="text-aurora-text-primary">{email}</span></p> : null}
        <TeamInvitationForm />
      </div>
    </div>
  )
}

function TeamInvitationForm() {
  const [token, setToken] = React.useState('')
  const [pending, setPending] = React.useState(false)
  const [error, setError] = React.useState<string | null>(null)
  const autoAccepted = React.useRef(false)

  const acceptInvitation = React.useCallback(async (rawToken: string) => {
    const invitationToken = normalizeTeamInvitationToken(rawToken)
    if (!invitationToken) {
      setError('Enter the 64-character invitation code from your administrator.')
      return
    }
    setPending(true)
    setError(null)
    try {
      const outcome = await accessApi.acceptTeamInvitation(invitationToken)
      if (outcome.organization_profile) {
        window.sessionStorage.setItem(
          ORGANIZATION_PROFILE_OFFER_SESSION_KEY,
          JSON.stringify(outcome.organization_profile),
        )
      }
      // Fragments are intentionally used for invite links so the secret never
      // reaches HTTP logs or referrer headers. Remove it once consumed.
      window.sessionStorage.removeItem(TEAM_INVITATION_TOKEN_SESSION_KEY)
      window.history.replaceState(null, '', window.location.pathname + window.location.search)
      await loadBrowserSession()
    } catch (caught) {
      if (caught instanceof AccessApiError && caught.code === 'validation_failed') {
        setError('That invitation code is not valid. Copy the complete code and try again.')
      } else if (caught instanceof AccessApiError && caught.code === 'forbidden') {
        setError('This invitation cannot be used by the account you signed in with, or it is no longer active.')
      } else {
        setError(caught instanceof Error ? caught.message : 'Labby could not accept this invitation.')
      }
    } finally {
      setPending(false)
    }
  }, [])

  React.useEffect(() => {
    if (autoAccepted.current || typeof window === 'undefined') return
    const invitationToken =
      teamInvitationTokenFromHash(window.location.hash) ??
      normalizeTeamInvitationToken(window.sessionStorage.getItem(TEAM_INVITATION_TOKEN_SESSION_KEY))
    if (!invitationToken) return
    autoAccepted.current = true
    setToken(invitationToken)
    void acceptInvitation(invitationToken)
  }, [acceptInvitation])

  function handleSubmit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    void acceptInvitation(token)
  }

  return (
    <form className="mt-6" onSubmit={handleSubmit}>
      <label className="block text-sm font-medium text-aurora-text-primary" htmlFor="team-invitation-token">
        Invitation code
      </label>
      <input
        autoComplete="off"
        autoFocus
        className={cn(FIELD_INPUT, 'font-mono')}
        id="team-invitation-token"
        inputMode="text"
        maxLength={64}
        onChange={(event) => setToken(event.target.value.replace(/\s/g, ''))}
        placeholder="Paste invitation code"
        spellCheck={false}
        value={token}
      />
      <p className="mt-2 text-xs leading-[1.5] text-aurora-text-muted">
        Open the invitation link from your administrator and Labby will fill this automatically. The code is matched to your verified sign-in email.
      </p>
      {error ? <ErrorNotice message={error} /> : null}
      <button className={PRIMARY_BUTTON} disabled={pending || !token.trim()} type="submit">
        {pending ? 'Joining team…' : 'Join team'}
      </button>
      <div className="mt-3 text-center">
        <SignOutButton compact />
      </div>
    </form>
  )
}

function SignOutButton({ compact = false }: { compact?: boolean }) {
  const [pending, setPending] = React.useState(false)
  const [error, setError] = React.useState<string | null>(null)

  async function handleSignOut() {
    setPending(true)
    setError(null)
    try {
      await logoutBrowserSession()
    } catch (reason) {
      // Local sign-out already completed; only server revocation failed.
      setError(reason instanceof LogoutRevocationError ? reason.message : 'Sign-out could not be confirmed by the server.')
    } finally {
      setPending(false)
    }
  }

  return (
    <>
      {error ? <ErrorNotice message={error} /> : null}
      <button
        className={compact ? 'inline-flex h-8 items-center justify-center text-sm text-aurora-text-muted underline-offset-4 hover:text-aurora-text-primary hover:underline' : PRIMARY_BUTTON}
        disabled={pending}
        onClick={handleSignOut}
        type="button"
      >
        {pending ? 'Signing out…' : 'Sign out'}
      </button>
    </>
  )
}

function ErrorNotice({ message }: { message: string }) {
  return (
    <div
      className="mt-4 rounded-aurora-2 border border-aurora-warn/30 bg-aurora-warn/10 px-4 py-3 text-sm text-aurora-warn"
      role="alert"
    >
      {message}
    </div>
  )
}
