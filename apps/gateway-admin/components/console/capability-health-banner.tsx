'use client'

import { useCallback, useEffect, useMemo, useState } from 'react'
import Link from 'next/link'
import { AlertTriangle, RefreshCw } from 'lucide-react'

import { doctorApi, type DoctorFinding } from '@/lib/api/doctor-client'

const REFRESH_MS = 60_000

export function CapabilityHealthBanner(): React.ReactElement | null {
  const [findings, setFindings] = useState<DoctorFinding[]>([])
  const [error, setError] = useState<string | undefined>()
  const [refreshNonce, setRefreshNonce] = useState(0)

  const refresh = useCallback(() => setRefreshNonce((value) => value + 1), [])

  useEffect(() => {
    const controller = new AbortController()
    const load = async () => {
      try {
        const report = await doctorApi.capabilityStatus(controller.signal)
        if (controller.signal.aborted) return
        setFindings(report.findings)
        setError(undefined)
      } catch (caught) {
        if (controller.signal.aborted) return
        setFindings([])
        setError(caught instanceof Error ? caught.message : 'capability health request failed')
      }
    }

    void load()
    const interval = setInterval(() => void load(), REFRESH_MS)
    return () => {
      controller.abort()
      clearInterval(interval)
    }
  }, [refreshNonce])

  const degraded = useMemo(
    () => findings.filter((finding) => finding.severity !== 'ok'),
    [findings],
  )

  if (!error && degraded.length === 0) return null

  const title = error
    ? 'Capability health is unavailable'
    : `Labby is running with ${degraded.length} degraded ${degraded.length === 1 ? 'capability' : 'capabilities'}`
  const details = error
    ? [`Labby could not verify which features are degraded: ${error}`]
    : degraded.slice(0, 3).map((finding) =>
        finding.check ? `${finding.check}: ${finding.message}` : finding.message,
      )

  return (
    <section
      role="status"
      aria-live="polite"
      data-capability-health="degraded"
      style={{
        width: '100%',
        border: '1px solid color-mix(in srgb, var(--aurora-warn) 48%, var(--aurora-border-default))',
        borderRadius: 'var(--radius-2)',
        background: 'color-mix(in srgb, var(--aurora-warn) 7%, var(--aurora-panel-strong))',
        color: 'var(--aurora-text-primary)',
        padding: '10px 12px',
        display: 'flex',
        alignItems: 'flex-start',
        gap: 10,
      }}
    >
      <AlertTriangle size={16} style={{ color: 'var(--aurora-warn)', flexShrink: 0, marginTop: 1 }} />
      <div style={{ minWidth: 0, flex: 1 }}>
        <div style={{ fontSize: 12, fontWeight: 700 }}>{title}</div>
        <div style={{ marginTop: 3, fontSize: 11.5, lineHeight: 1.45, color: 'var(--aurora-text-muted)' }}>
          {details.join(' · ')}
          {degraded.length > 3 ? ` · +${degraded.length - 3} more` : ''}
        </div>
        <div style={{ marginTop: 6, display: 'flex', alignItems: 'center', gap: 10, fontSize: 11.5 }}>
          <Link href="/settings/doctor" style={{ color: 'var(--aurora-accent-primary)', fontWeight: 650 }}>
            Open Doctor
          </Link>
          <button
            type="button"
            onClick={refresh}
            aria-label="Refresh capability health"
            style={{
              display: 'inline-flex',
              alignItems: 'center',
              gap: 4,
              border: 0,
              padding: 0,
              background: 'transparent',
              color: 'var(--aurora-text-muted)',
              cursor: 'pointer',
              font: 'inherit',
            }}
          >
            <RefreshCw size={11} /> refresh
          </button>
        </div>
      </div>
    </section>
  )
}
