'use client'

import { Button } from '../ui/button.tsx'
import { LabbyIcon } from '../labby-icon.tsx'
import {
  AURORA_DISPLAY_1,
  AURORA_MUTED_LABEL,
  AURORA_PAGE_SHELL,
  AURORA_STRONG_PANEL,
} from '../aurora/tokens.ts'
import { cn } from '../../lib/utils.ts'

type LoginScreenProps = {
  errorMessage?: string
  requestId?: string
  returnTo: string
}

export function LoginScreen({ errorMessage, requestId, returnTo }: LoginScreenProps) {
  const introCopy = errorMessage
    ? 'Labby could not verify your current session. Try signing in again.'
    : 'Sign in to access your Labby workspace.'

  return (
    <div className={cn(AURORA_PAGE_SHELL, 'flex min-h-screen items-center justify-center px-6')}>
      <div className={cn(AURORA_STRONG_PANEL, 'w-full max-w-md p-8')}>
        <div className="flex items-center gap-3 mb-6">
          <LabbyIcon size={40} />
          <span className="text-xl font-bold text-aurora-text-primary">Labby</span>
        </div>
        <p className={AURORA_MUTED_LABEL}>
          {errorMessage ? 'Authentication Error' : 'Authentication Required'}
        </p>
        <h1 className={cn(AURORA_DISPLAY_1, 'mt-3 text-aurora-text-primary')}>Sign In to Labby</h1>
        <p className="mt-3 text-sm leading-[1.55] text-aurora-text-muted">{introCopy}</p>
        {errorMessage ? (
          <div className="mt-6 rounded-aurora-2 border border-aurora-warn/30 bg-aurora-warn/10 px-4 py-3 text-sm text-aurora-warn">
            <p>{errorMessage}</p>
            {requestId ? (
              <p className="mt-2 text-xs text-aurora-warn/80">Request ID: {requestId}</p>
            ) : null}
          </div>
        ) : null}
        <Button
          size="lg"
          className="mt-8 w-full"
          onClick={() => {
            window.location.assign(`/auth/login?return_to=${encodeURIComponent(returnTo)}`)
          }}
          type="button"
        >
          {errorMessage ? 'Sign In Again' : 'Sign In'}
        </Button>
      </div>
    </div>
  )
}
