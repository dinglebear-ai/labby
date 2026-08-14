'use client'

// Extract panel — runs `extract.scan` (read-only) and lets the operator
// "Apply to draft" any subset of discovered credentials. Each Apply is a
// batched setup.draft.set scoped to the selected service; commit is
// driven from the per-service settings page.

import { useState } from 'react'
import { Loader2, RefreshCw, ArrowRight, CheckCircle2 } from 'lucide-react'

import { Button } from '@/components/ui/button'
import {
  SettingsCard,
  SettingsRow,
  SettingsRowStrip,
} from '@/components/settings/SettingsChrome'
import { Checkbox } from '@/components/ui/checkbox'
import { extractApi, type ExtractCredential, type ExtractReport } from '@/lib/api/extract-client'
import { setupApi } from '@/lib/api/setup-client'

export default function ExtractPanel(): React.ReactElement {
  const [report, setReport] = useState<ExtractReport | undefined>()
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | undefined>()
  const [selected, setSelected] = useState<Set<number>>(new Set())
  const [applying, setApplying] = useState(false)
  const [appliedCount, setAppliedCount] = useState<number | undefined>()

  async function rescan(): Promise<void> {
    setLoading(true)
    setError(undefined)
    setAppliedCount(undefined)
    try {
      const result = await extractApi.scan()
      setReport(result)
      setSelected(new Set(result.creds.map((_, i) => i)))
    } catch (err) {
      setError(err instanceof Error ? err.message : 'extract.scan failed')
    } finally {
      setLoading(false)
    }
  }

  function toggle(idx: number): void {
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(idx)) next.delete(idx)
      else next.add(idx)
      return next
    })
  }

  async function applyToDraft(): Promise<void> {
    if (!report) return
    const entries: { key: string; value: string }[] = []
    for (const idx of selected) {
      const cred = report.creds[idx]
      if (!cred) continue
      const upper = cred.service.toUpperCase()
      if (cred.url) entries.push({ key: `${upper}_URL`, value: cred.url })
      // The redacted scan response sets secret_present but does not return
      // the value, so we cannot batch the secrets here. The operator must
      // re-enter them in the per-service settings page.
    }
    if (entries.length === 0) {
      setAppliedCount(0)
      return
    }
    setApplying(true)
    try {
      const result = await setupApi.draftSet(entries, { force: true })
      setAppliedCount(result.written)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'draft.set failed')
    } finally {
      setApplying(false)
    }
  }

  const previewEntries = report
    ? [...selected]
        .map((idx) => report.creds[idx])
        .filter((cred): cred is ExtractCredential => Boolean(cred?.url))
        .map((cred) => ({ key: `${cred.service.toUpperCase()}_URL`, value: cred.url! }))
    : []

  return (
    <>
      <h2 className="sr-only">Extract settings</h2>
      <SettingsCard
        title="Extract"
        description={
          <>
            Scan local + SSH hosts for existing service credentials and apply
            the discovered URLs to your draft. Secret values are redacted in
            transit; re-enter them in each service&apos;s settings page.
          </>
        }
        action={
          <Button variant="outline" size="sm" onClick={rescan} disabled={loading}>
            <RefreshCw className={`mr-2 h-3 w-3 ${loading ? 'animate-spin' : ''}`} />
            {report ? 'Re-scan' : 'Scan'}
          </Button>
        }
      >
        {loading ? (
          <SettingsRowStrip>
            <span className="flex items-center gap-2 text-[11.5px] text-aurora-text-muted">
              <Loader2 className="h-4 w-4 animate-spin" /> running extract.scan
            </span>
          </SettingsRowStrip>
        ) : null}
        {error ? (
          <SettingsRowStrip>
            <span className="text-[11.5px] text-destructive">{error}</span>
          </SettingsRowStrip>
        ) : null}

        {report && report.creds.length === 0 ? (
          <SettingsRowStrip>
            <span style={{ fontSize: 11.5, color: 'var(--aurora-text-muted)' }}>
              No credentials discovered.
            </span>
          </SettingsRowStrip>
        ) : null}

        {report && report.creds.length > 0 ? (
          <>
            {report.creds.map((cred, idx) => (
              <SettingsRow
                key={`${cred.service}-${idx}`}
                label={cred.service}
                description={
                  <>
                    {cred.url ? <span style={{ display: 'block' }}>URL: {cred.url}</span> : null}
                    <span style={{ display: 'block' }}>
                      {cred.secret_present ? 'Secret present (redacted)' : 'No secret'}
                      {cred.source_host ? ` — host: ${cred.source_host}` : ''}
                    </span>
                  </>
                }
                control={
                  <Checkbox
                    checked={selected.has(idx)}
                    onCheckedChange={() => toggle(idx)}
                    aria-label={`Toggle ${cred.service}`}
                  />
                }
              />
            ))}
            <SettingsRow
              layout="stacked"
              label="Draft preview"
              description="Redacted secrets are not written by extract; enter them on each service page."
              control={
                previewEntries.length > 0 ? (
                  <ul style={{ display: 'grid', gap: 2 }}>
                    {previewEntries.map((entry) => (
                      <li
                        key={entry.key}
                        style={{ fontSize: 11, color: 'var(--aurora-text-muted)' }}
                      >
                        <code>
                          {entry.key} = {entry.value}
                        </code>
                      </li>
                    ))}
                  </ul>
                ) : (
                  <p style={{ margin: 0, fontSize: 11, color: 'var(--aurora-text-muted)' }}>
                    Selected credentials do not include writable URL values.
                  </p>
                )
              }
            />
            <SettingsRowStrip style={{ gap: 10 }}>
              <Button size="sm" onClick={applyToDraft} disabled={applying || selected.size === 0}>
                {applying ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}
                <ArrowRight className="mr-2 h-4 w-4" />
                Apply {selected.size} to draft
              </Button>
              {appliedCount !== undefined ? (
                <span
                  className="inline-flex items-center gap-1"
                  style={{ fontSize: 11, color: 'var(--aurora-success)' }}
                >
                  <CheckCircle2 className="h-3 w-3" /> {appliedCount} entries written
                </span>
              ) : null}
            </SettingsRowStrip>
          </>
        ) : null}

        {report?.warnings && report.warnings.length > 0 ? (
          <SettingsRowStrip>
            <ul
              className="list-disc pl-5"
              style={{ fontSize: 11, color: 'var(--aurora-warn)' }}
            >
              {report.warnings.map((w, i) => (
                <li key={i}>
                  {w.service ? `${w.service}: ` : ''}
                  {w.message}
                </li>
              ))}
            </ul>
          </SettingsRowStrip>
        ) : null}
      </SettingsCard>
    </>
  )
}

// Re-export the credential type for type clarity in this file (suppresses
// unused-import lints if extract-client.ts changes).
export type { ExtractCredential }
