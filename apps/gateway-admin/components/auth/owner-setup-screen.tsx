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
import { bootstrapOwner, describeOwnerBootstrapError } from '../../lib/auth/owner-bootstrap.ts'
import { LogoutRevocationError, logoutBrowserSession } from '../../lib/auth/session.ts'
import { requestTeamAdmission } from '../../lib/auth/team-admission.ts'
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
  unprovisioned: 'This identity is signed in but has no access yet. Ask an administrator to add it to a team.',
}

/**
 * Shown while the server reports a signed-in session without durable access
 * authority. Only when the server declares `owner_bootstrap_available` does
 * it render the form for `POST /v1/access/bootstrap-owner`; every other
 * caller sees a no-access notice. The server still enforces every gate.
 */
export function OwnerSetupScreen({ authorityState, bootstrapAvailable, remediation, email }: OwnerSetupScreenProps) {
  const heading = bootstrapAvailable ? 'Finish setting up Labby' : 'No access yet'
  // The transport remediation from the server describes bootstrap; only an
  // eligible caller should see it.
  const message = bootstrapAvailable
    ? remediation || BOOTSTRAP_REMEDIATION
    : (authorityState === 'unprovisioned' && remediation) || NO_ACCESS_REMEDIATION[authorityState]

  // Give the server's team admission policy one `/v1` request to act on; a
  // qualifying identity reloads as `ready` and this screen unmounts.
  const admissionRequested = React.useRef(false)
  const [admissionWarning, setAdmissionWarning] = React.useState<string | null>(null)
  React.useEffect(() => {
    if (authorityState !== 'unprovisioned' || bootstrapAvailable || admissionRequested.current) return
    admissionRequested.current = true
    void requestTeamAdmission()
      .then((result) => {
        if (result.admissionError) {
          setAdmissionWarning(`${result.admissionError} Your signed-in session was refreshed, but automatic team admission could not be confirmed.`)
        }
      })
      .catch((error) => {
        setAdmissionWarning(
          `Labby could not refresh your access state: ${error instanceof Error ? error.message : 'session refresh failed'}. Retry after checking the server connection or sign out and back in.`,
        )
      })
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
        {admissionWarning ? (
          <div role="alert" className="mt-4 rounded-md border border-aurora-warn/35 bg-aurora-warn/8 px-3 py-2 text-sm leading-[1.5] text-aurora-warn">
            {admissionWarning}
          </div>
        ) : null}
        {bootstrapAvailable ? <OwnerBootstrapForm /> : <SignOutButton />}
      </div>
    </div>
  )
}

function OwnerBootstrapForm() {
  const [organizationName, setOrganizationName] = React.useState('')
  const [projectName, setProjectName] = React.useState('')
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
        If this Labby was set up before, enter the organization and project names from that setup.
      </p>
      <label className="mt-4 block text-sm font-medium text-aurora-text-primary" htmlFor="owner-setup-organization">
        Organization name
      </label>
      <input
        autoComplete="off"
        className={FIELD_INPUT}
        id="owner-setup-organization"
        maxLength={128}
        name="organization_name"
        onChange={(event) => setOrganizationName(event.target.value)}
        placeholder="Local"
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
        placeholder="Default"
        required
        value={projectName}
      />
      {error ? <ErrorNotice message={error} /> : null}
      <button className={PRIMARY_BUTTON} disabled={pending} type="submit">
        {pending ? 'Completing owner bootstrap…' : 'Complete owner bootstrap'}
      </button>
    </form>
  )
}

function SignOutButton() {
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
      <button className={PRIMARY_BUTTON} disabled={pending} onClick={handleSignOut} type="button">
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
