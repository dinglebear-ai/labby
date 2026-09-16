'use client'

import * as React from 'react'

import { LabbyIcon } from '../labby-icon.tsx'
import { Button } from '../ui/button.tsx'
import { Input } from '../ui/input.tsx'
import {
  AURORA_DISPLAY_1,
  AURORA_MUTED_LABEL,
  AURORA_PAGE_SHELL,
  AURORA_STRONG_PANEL,
} from '../aurora/tokens.ts'
import { setupApi, type OrganizationBootstrapCreateResponse, type OrganizationBootstrapPreview } from '../../lib/api/setup-client.ts'
import {
  ORGANIZATION_PROFILE_IMPORT_SESSION_KEY,
  ORGANIZATION_PROFILE_OFFER_SESSION_KEY,
  normalizePersonalLabbyOrigin,
  organizationProfileHandoffHref,
} from '../../lib/auth/organization-profile-handoff.ts'
import { cn } from '../../lib/utils.ts'

export function TeamEnrollmentCompleteScreen({ offer }: { offer: OrganizationBootstrapCreateResponse }) {
  const [personalOrigin, setPersonalOrigin] = React.useState('http://127.0.0.1:8765')
  const [error, setError] = React.useState<string>()
  const normalizedOrigin = normalizePersonalLabbyOrigin(personalOrigin)

  function handoffHref() {
    try {
      return organizationProfileHandoffHref(personalOrigin, offer)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Could not create the personal Labby handoff.')
      return undefined
    }
  }

  function openPersonalLabby() {
    const href = handoffHref()
    if (href) window.location.assign(href)
  }

  async function copyHandoff() {
    const href = handoffHref()
    if (!href) return
    try {
      await navigator.clipboard.writeText(href)
      setError(undefined)
    } catch {
      setError('Copy failed. You can edit the personal Labby URL and use Open personal Labby instead.')
    }
  }

  function continueWithoutImport() {
    window.sessionStorage.removeItem(ORGANIZATION_PROFILE_OFFER_SESSION_KEY)
    window.location.reload()
  }

  return (
    <Shell eyebrow="Team joined" title="Bring team defaults into your personal Labby">
      <p className="text-sm leading-[1.6] text-aurora-text-muted">
        Your Team membership is active. The organization also published a signed, non-secret setup profile for Team Depot and approved integrations. Your personal Labby stays the runtime owner.
      </p>
      <div className="mt-5 rounded-aurora-2 border border-aurora-border-subtle bg-aurora-control-surface p-4">
        <p className={AURORA_MUTED_LABEL}>Signer fingerprint</p>
        <code className="mt-1 block break-all text-xs text-aurora-text-primary">{offer.signer_fingerprint}</code>
      </div>
      <label className="mt-5 block text-sm font-medium text-aurora-text-primary" htmlFor="personal-labby-origin">
        Personal Labby URL
      </label>
      <Input
        id="personal-labby-origin"
        className="mt-1.5"
        value={personalOrigin}
        onChange={(event) => { setPersonalOrigin(event.target.value); setError(undefined) }}
        placeholder="http://127.0.0.1:8765"
      />
      <p className="mt-2 text-xs leading-5 text-aurora-text-muted">
        Local Labby uses 127.0.0.1 by default. Remote personal Labby URLs must use HTTPS. The signed profile travels only in the browser fragment, so it is not sent to the Team server or a reverse proxy.
      </p>
      {error ? <Notice message={error} /> : null}
      <div className="mt-5 grid gap-2 sm:grid-cols-2">
        <Button disabled={!normalizedOrigin} onClick={openPersonalLabby}>Open personal Labby</Button>
        <Button variant="outline" disabled={!normalizedOrigin} onClick={() => void copyHandoff()}>Copy handoff link</Button>
      </div>
      <button className="mt-4 text-sm text-aurora-text-muted underline-offset-4 hover:text-aurora-text-primary hover:underline" onClick={continueWithoutImport} type="button">
        Continue without importing team defaults
      </button>
    </Shell>
  )
}

export function OrganizationProfileImportScreen({ offer }: { offer: OrganizationBootstrapCreateResponse }) {
  const [preview, setPreview] = React.useState<OrganizationBootstrapPreview | null>(null)
  const [loading, setLoading] = React.useState(true)
  const [applying, setApplying] = React.useState(false)
  const [error, setError] = React.useState<string>()

  React.useEffect(() => {
    const controller = new AbortController()
    setupApi.organizationProfilePreview(offer.profile, controller.signal)
      .then((value) => { if (!controller.signal.aborted) setPreview(value) })
      .catch((cause) => { if (!controller.signal.aborted) setError(cause instanceof Error ? cause.message : 'Could not verify the organization profile.') })
      .finally(() => { if (!controller.signal.aborted) setLoading(false) })
    return () => controller.abort()
  }, [offer])

  async function applyProfile() {
    setApplying(true)
    setError(undefined)
    try {
      await setupApi.organizationProfileApply(offer.profile, offer.signer_fingerprint)
      window.sessionStorage.removeItem(ORGANIZATION_PROFILE_IMPORT_SESSION_KEY)
      window.location.reload()
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Could not apply the organization profile.')
      setApplying(false)
    }
  }

  function skip() {
    window.sessionStorage.removeItem(ORGANIZATION_PROFILE_IMPORT_SESSION_KEY)
    window.location.reload()
  }

  return (
    <Shell eyebrow="Organization profile" title="Add your team defaults">
      {loading ? (
        <p className="text-sm text-aurora-text-muted">Verifying the signed profile…</p>
      ) : preview ? (
        <>
          <p className="text-sm leading-[1.6] text-aurora-text-muted">
            Labby verified the profile signature. Applying it adds these integrations to this personal Labby. It does not transfer your credentials or runtime ownership to Team Labby.
          </p>
          <div className="mt-5 grid gap-2">
            {preview.integrations.map((integration) => (
              <div key={integration.name} className="rounded-aurora-2 border border-aurora-border-subtle bg-aurora-control-surface px-4 py-3">
                <p className="font-medium text-aurora-text-primary">{integration.display_name}</p>
                <p className="mt-1 break-all text-xs text-aurora-text-muted">{integration.url}</p>
              </div>
            ))}
          </div>
          <div className="mt-4 rounded-aurora-2 border border-aurora-border-subtle p-4">
            <p className={AURORA_MUTED_LABEL}>Trusted signer fingerprint</p>
            <code className="mt-1 block break-all text-xs text-aurora-text-primary">{preview.signer_fingerprint}</code>
          </div>
          {preview.signer_fingerprint !== offer.signer_fingerprint ? (
            <Notice message="The verified signer fingerprint does not match the Team handoff. Nothing will be applied." />
          ) : null}
          {error ? <Notice message={error} /> : null}
          <Button
            className="mt-5 w-full"
            disabled={applying || preview.signer_fingerprint !== offer.signer_fingerprint}
            onClick={() => void applyProfile()}
          >
            {applying ? 'Applying team defaults…' : 'Trust team and apply defaults'}
          </Button>
          <button className="mt-3 w-full text-sm text-aurora-text-muted underline-offset-4 hover:text-aurora-text-primary hover:underline" onClick={skip} type="button">
            Continue without team defaults
          </button>
        </>
      ) : (
        <>
          <Notice message={error ?? 'The organization profile could not be verified.'} />
          <Button className="mt-4 w-full" variant="outline" onClick={skip}>Continue without team defaults</Button>
        </>
      )}
    </Shell>
  )
}

function Shell({ eyebrow, title, children }: { eyebrow: string; title: string; children: React.ReactNode }) {
  return (
    <div className={cn(AURORA_PAGE_SHELL, 'flex min-h-screen items-center justify-center px-6 py-8')}>
      <div className={cn(AURORA_STRONG_PANEL, 'w-full max-w-lg p-8')}>
        <div className="mb-6 flex items-center gap-3">
          <LabbyIcon size={40} />
          <span className="text-xl font-bold text-aurora-text-primary">Labby</span>
        </div>
        <p className={AURORA_MUTED_LABEL}>{eyebrow}</p>
        <h1 className={cn(AURORA_DISPLAY_1, 'mt-3 text-aurora-text-primary')}>{title}</h1>
        <div className="mt-4">{children}</div>
      </div>
    </div>
  )
}

function Notice({ message }: { message: string }) {
  return <p className="mt-4 rounded-aurora-2 border border-aurora-warn/30 bg-aurora-warn/10 px-4 py-3 text-sm text-aurora-warn">{message}</p>
}
