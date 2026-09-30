'use client'

// Polls setup.state on mount + on window.focus (debounced 1s trailing
// edge + AbortController dedup) and renders a non-blocking warning
// banner when `draft_stale: true`.

import { useEffect, useState } from 'react'
import { AlertTriangle } from 'lucide-react'

import { Button } from '@/components/ui/button'
import { setupApi, type SetupSnapshot } from '@/lib/api/setup-client'

type Status = 'unknown' | 'fresh' | 'stale' | 'unavailable'

export function DraftStaleBanner(): React.ReactElement | null {
  const [status, setStatus] = useState<Status>('unknown')
  const [snapshot, setSnapshot] = useState<SetupSnapshot | null>(null)
  const [discarding, setDiscarding] = useState(false)
  const [discardError, setDiscardError] = useState<string | null>(null)

  async function refresh(signal?: AbortSignal): Promise<void> {
    const snapshot = await setupApi.state(signal)
    setSnapshot(snapshot)
    setStatus(snapshot.draft_stale ? 'stale' : 'fresh')
  }

  useEffect(() => {
    let cancelled = false
    let inFlight: AbortController | null = null
    let debounceTimer: ReturnType<typeof setTimeout> | null = null

    async function check(): Promise<void> {
      inFlight?.abort()
      const controller = new AbortController()
      inFlight = controller
      try {
        const snapshot = await setupApi.state(controller.signal)
        if (cancelled || controller.signal.aborted) return
        setSnapshot(snapshot)
        setStatus(snapshot.draft_stale ? 'stale' : 'fresh')
      } catch (err) {
        if (cancelled || controller.signal.aborted) return
        // AbortError is expected churn (a newer check superseded this one)
        // and is silent. Anything else means the gateway is unreachable
        // or returning errors — surface that as 'unavailable' so users
        // know draft-stale detection is offline rather than silently
        // assuming everything is fine.
        if (err instanceof Error && err.name === 'AbortError') return
        console.warn('DraftStaleBanner: setup.state failed', err)
        setStatus('unavailable')
      }
    }

    function schedule(): void {
      if (debounceTimer) clearTimeout(debounceTimer)
      debounceTimer = setTimeout(() => {
        void check()
      }, 1000)
    }

    function onVisibility(): void {
      if (document.visibilityState === 'visible') schedule()
    }

    void check()
    window.addEventListener('focus', schedule)
    // visibilitychange covers tab-switch on browsers where 'focus' doesn't
    // fire when switching between tabs in the same window (Chrome on
    // mobile, some multi-tab desktop workflows).
    document.addEventListener('visibilitychange', onVisibility)
    return () => {
      cancelled = true
      window.removeEventListener('focus', schedule)
      document.removeEventListener('visibilitychange', onVisibility)
      if (debounceTimer) clearTimeout(debounceTimer)
      inFlight?.abort()
    }
  }, [])

  async function discardDraft(): Promise<void> {
    setDiscarding(true)
    setDiscardError(null)
    try {
      await setupApi.draftDiscard()
      await refresh()
    } catch (err) {
      setDiscardError(err instanceof Error ? err.message : 'Could not discard the draft.')
    } finally {
      setDiscarding(false)
    }
  }

  if (status === 'unknown' || status === 'fresh') return null
  if (status === 'unavailable') {
    return (
      <div style={{ ...BANNER_STYLE, ...NEUTRAL_BANNER_TONE }}>
        <AlertTriangle size={14} style={{ marginTop: 2, flexShrink: 0 }} />
        <div style={{ minWidth: 0, flex: 1 }}>
          <p style={BANNER_TITLE_STYLE}>Draft state check unavailable.</p>
          <p style={BANNER_BODY_STYLE}>
            Could not reach the lab gateway. Concurrent-edit detection is
            offline — saving here may overwrite changes from another session
            without warning.
          </p>
        </div>
      </div>
    )
  }
  const draftSummary = staleDraftSummary(snapshot)
  return (
    <div style={{ ...BANNER_STYLE, ...WARN_BANNER_TONE }}>
      <AlertTriangle size={14} style={{ marginTop: 2, flexShrink: 0, color: 'var(--aurora-warn)' }} />
      <div style={{ minWidth: 0, flex: 1 }}>
        <p style={BANNER_TITLE_STYLE}>Old draft detected.</p>
        <p style={BANNER_BODY_STYLE}>
          A saved setup draft{draftSummary} is older than <code>~/.labby/.env</code>. Discard it if
          you do not need those draft values.
        </p>
        {discardError ? <p style={{ ...BANNER_BODY_STYLE, marginTop: 6 }}>{discardError}</p> : null}
      </div>
      <Button
        type="button"
        size="sm"
        variant="outline"
        onClick={() => void discardDraft()}
        disabled={discarding}
        className="shrink-0 border-aurora-warn/40 bg-transparent text-aurora-text-primary hover:bg-aurora-warn/15"
      >
        {discarding ? 'Discarding...' : 'Discard draft'}
      </Button>
    </div>
  )
}

// Banner chrome tracks the mock's settings cards: --radius-2, the same 16px
// row inset, and the row label/description type scale.
const BANNER_STYLE: React.CSSProperties = {
  display: 'flex',
  alignItems: 'flex-start',
  gap: 10,
  padding: '11px 16px',
  borderRadius: 'var(--radius-2)',
}

const NEUTRAL_BANNER_TONE: React.CSSProperties = {
  border:
    '1px solid color-mix(in srgb, var(--aurora-border-default) 70%, var(--aurora-page-bg))',
  background: 'var(--gw0-0_38)',
  color: 'var(--aurora-text-muted)',
}

const WARN_BANNER_TONE: React.CSSProperties = {
  border: '1px solid color-mix(in srgb, var(--aurora-warn) 30%, transparent)',
  background: 'color-mix(in srgb, var(--aurora-warn) 10%, transparent)',
  color: 'var(--aurora-text-muted)',
}

const BANNER_TITLE_STYLE: React.CSSProperties = {
  margin: 0,
  fontSize: 13,
  fontWeight: 600,
  color: 'var(--aurora-text-primary)',
}

const BANNER_BODY_STYLE: React.CSSProperties = {
  margin: '2px 0 0',
  fontSize: 11.5,
  lineHeight: 1.5,
  color: 'var(--aurora-text-muted)',
}

function staleDraftSummary(snapshot: SetupSnapshot | null): string {
  if (!snapshot) return ''
  const pieces: string[] = []
  if (snapshot.draft_entry_count > 0) {
    pieces.push(
      `${snapshot.draft_entry_count} value${snapshot.draft_entry_count === 1 ? '' : 's'}`,
    )
  }
  const date = formatUnixDate(snapshot.draft_mtime_unix_seconds)
  if (date) pieces.push(`from ${date}`)
  return pieces.length > 0 ? ` with ${pieces.join(' ')}` : ''
}

function formatUnixDate(seconds: number | null): string | null {
  if (typeof seconds !== 'number' || !Number.isFinite(seconds) || seconds <= 0) return null
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
    year: 'numeric',
  }).format(new Date(seconds * 1000))
}
