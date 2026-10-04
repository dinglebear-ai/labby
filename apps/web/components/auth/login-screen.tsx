'use client'

import { FormEvent, useEffect, useState } from 'react'

import { Button } from '../ui/button.tsx'
import { LabbyIcon } from '../labby-icon.tsx'
import {
  AURORA_DISPLAY_1,
  AURORA_MUTED_LABEL,
  AURORA_PAGE_SHELL,
  AURORA_STRONG_PANEL,
} from '../aurora/tokens.ts'
import { cn } from '../../lib/utils.ts'
import { redeemSetupBrowserHandoff } from '../../lib/auth/session-store.ts'
import { exchangeBearerBrowserSession, useBrowserSession } from '../../lib/auth/session.ts'

type LoginScreenProps = {
  errorMessage?: string
  requestId?: string
  returnTo: string
}

export function LoginScreen({ errorMessage, requestId, returnTo }: LoginScreenProps) {
  const session = useBrowserSession()
  const [token, setToken] = useState('')
  const [bearerError, setBearerError] = useState<string>()
  const [submitting, setSubmitting] = useState(false)
  useEffect(() => {
    const fragment = new URLSearchParams(window.location.hash.slice(1))
    const handoff = fragment.get('setup_handoff')
    if (!handoff) return
    // Remove the capability before any request, telemetry, render, or navigation.
    window.history.replaceState(window.history.state, '', `${window.location.pathname}${window.location.search}`)
    setSubmitting(true)
    void redeemSetupBrowserHandoff(handoff).then(next => {
      if (next.status !== 'authenticated') throw new Error('Labby could not establish the setup browser session. Run setup again.')
    }).catch(error => setBearerError(error instanceof Error ? error.message : 'Local setup handoff could not be verified.'))
      .finally(() => setSubmitting(false))
  }, [])
  const loginAvailable = session.status === 'unauthenticated' && session.loginAvailable === true
  const bearerAvailable = session.status === 'unauthenticated' && session.bearerLoginAvailable === true
  const authUnavailable = session.status === 'auth_error'

  const introCopy = errorMessage
    ? 'Labby could not verify your current session.'
    : bearerAvailable && loginAvailable
      ? 'Use your configured sign-in provider or an operator bearer token.'
      : bearerAvailable
        ? 'Local setup opens this page with a secure sign-in link. For manual operator access, enter your gateway bearer token.'
        : 'Sign in to access your Labby workspace.'

  async function submitBearer(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (submitting) return
    setBearerError(undefined)
    setSubmitting(true)
    try {
      const next = await exchangeBearerBrowserSession(token)
      if (next.status !== 'authenticated') {
        throw new Error('Labby accepted the exchange but did not create a browser session.')
      }
    } catch (error) {
      setBearerError(error instanceof Error ? error.message : 'Labby rejected that bearer token.')
    } finally {
      // The setup token is a long-lived operator credential; whether the
      // exchange succeeded or was refused, it must not linger in the form.
      setToken('')
      setSubmitting(false)
    }
  }

  return (
    <div className={cn(AURORA_PAGE_SHELL, 'flex min-h-screen items-center justify-center px-4 py-8 sm:px-6')}>
      <div className={cn(AURORA_STRONG_PANEL, 'w-full max-w-sm p-6 sm:p-7')}>
        <div className="mb-5 flex items-center gap-3">
          <LabbyIcon size={34} />
          <div className="min-w-0">
            <span className="block text-lg font-bold leading-none text-aurora-text-primary">Labby</span>
            <span className="mt-1 block text-xs text-aurora-text-muted">Control Plane</span>
          </div>
        </div>
        <p className={AURORA_MUTED_LABEL}>{errorMessage ? 'Authentication Error' : 'Authentication Required'}</p>
        <h1 className={cn(AURORA_DISPLAY_1, 'mt-2 text-aurora-text-primary')}>Sign in</h1>
        <p className="mt-2 text-sm leading-[1.5] text-aurora-text-muted">{introCopy}</p>

        {errorMessage ? (
          <div className="mt-4 rounded-aurora-2 border border-aurora-warn/30 bg-aurora-warn/10 px-3 py-2.5 text-sm text-aurora-warn">
            <p>{errorMessage}</p>
            {requestId ? <p className="mt-1.5 text-xs text-aurora-warn/80">Request ID: {requestId}</p> : null}
          </div>
        ) : null}

        {bearerAvailable ? (
          <form className="mt-5" onSubmit={submitBearer}>
            <label className="block text-xs font-semibold text-aurora-text-muted" htmlFor="labby-bearer-token">
              Operator bearer token
            </label>
            <input
              id="labby-bearer-token"
              type="password"
              autoComplete="off"
              spellCheck={false}
              value={token}
              onChange={(event) => setToken(event.currentTarget.value)}
              placeholder="Paste bearer token"
              className="mt-2 h-11 w-full rounded-aurora-2 border border-aurora-border bg-aurora-raised px-3 font-mono text-sm text-aurora-text-primary outline-none transition focus:border-aurora-accent focus:ring-2 focus:ring-aurora-accent/20"
            />
            {bearerError ? <p className="mt-2 text-xs text-aurora-warn">{bearerError}</p> : null}
            <Button className="mt-3 w-full" size="lg" disabled={submitting || token.trim().length === 0} type="submit">
              {submitting ? 'Signing in…' : 'Continue'}
            </Button>
          </form>
        ) : null}

        {bearerAvailable && loginAvailable ? (
          <div className="my-4 flex items-center gap-3 text-[10px] font-semibold uppercase tracking-[0.12em] text-aurora-text-muted">
            <span className="h-px flex-1 bg-aurora-border" />or<span className="h-px flex-1 bg-aurora-border" />
          </div>
        ) : null}

        {loginAvailable ? (
          <Button
            size="lg"
            className={cn('w-full', bearerAvailable ? '' : 'mt-6')}
            onClick={() => {
              window.location.assign(`/auth/login?return_to=${encodeURIComponent(returnTo)}`)
            }}
            type="button"
          >
            Continue with sign-in provider
          </Button>
        ) : null}

        {authUnavailable ? (
          <Button className="mt-5 w-full" size="lg" onClick={() => window.location.reload()} type="button">
            Sign In Again
          </Button>
        ) : null}

        {!authUnavailable && !bearerAvailable && !loginAvailable ? (
          <p className="mt-5 rounded-aurora-2 border border-aurora-border bg-aurora-raised px-3 py-2.5 text-xs leading-relaxed text-aurora-text-muted">
            This server has no interactive browser sign-in method available. Check the Labby server authentication configuration.
          </p>
        ) : null}
      </div>
    </div>
  )
}
