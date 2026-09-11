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
import { cn } from '../../lib/utils.ts'

type OwnerSetupScreenProps = {
  authorityState: 'transport' | 'unprovisioned'
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

const FALLBACK_REMEDIATION: Record<OwnerSetupScreenProps['authorityState'], string> = {
  transport: 'Durable access authority is not initialized yet. Complete owner bootstrap to finish setup.',
  unprovisioned: 'This identity is signed in but has no access yet. Ask an administrator to add it to a team, or complete owner bootstrap.',
}

/**
 * Shown while the server reports a signed-in session without durable access
 * authority. It is the browser surface for `POST /v1/access/bootstrap-owner`;
 * the server enforces every eligibility gate.
 */
export function OwnerSetupScreen({ authorityState, remediation, email }: OwnerSetupScreenProps) {
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
    <div className={cn(AURORA_PAGE_SHELL, 'flex min-h-screen items-center justify-center px-6')}>
      <div className={cn(AURORA_STRONG_PANEL, 'w-full max-w-md p-8')}>
        <div className="mb-6 flex items-center gap-3">
          <LabbyIcon size={40} />
          <span className="text-xl font-bold text-aurora-text-primary">Labby</span>
        </div>
        <p className={AURORA_MUTED_LABEL}>Owner setup</p>
        <h1 className={cn(AURORA_DISPLAY_1, 'mt-3 text-aurora-text-primary')}>
          {authorityState === 'transport' ? 'Finish setting up Labby' : 'No access yet'}
        </h1>
        <p className="mt-3 text-sm leading-[1.55] text-aurora-text-muted">
          {remediation || FALLBACK_REMEDIATION[authorityState]}
        </p>
        {email ? (
          <p className="mt-2 text-sm text-aurora-text-muted">
            Signed in as <span className="text-aurora-text-primary">{email}</span>
          </p>
        ) : null}
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
          {error ? (
            <div
              className="mt-4 rounded-aurora-2 border border-aurora-warn/30 bg-aurora-warn/10 px-4 py-3 text-sm text-aurora-warn"
              role="alert"
            >
              {error}
            </div>
          ) : null}
          <button className={PRIMARY_BUTTON} disabled={pending} type="submit">
            {pending ? 'Completing owner bootstrap…' : 'Complete owner bootstrap'}
          </button>
        </form>
      </div>
    </div>
  )
}
