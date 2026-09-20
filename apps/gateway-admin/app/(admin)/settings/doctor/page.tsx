'use client'

import { useCallback, useEffect, useMemo, useState } from 'react'
import { Copy, ExternalLink, KeyRound, LifeBuoy, Loader2, PlugZap, RefreshCw, ShieldCheck } from 'lucide-react'
// AppHeader is owned by the parent /settings/layout.tsx — do not double-mount.
import {
  SettingsCard,
  SettingsRow,
  SettingsRowStrip,
  SettingsValue,
} from '@/components/settings/SettingsChrome'
import { hasMockDataAuthMode, isStandaloneBearerAuthMode } from '@/lib/auth/auth-mode'
import { buildGatewaySettingsSnapshot } from '@/lib/dashboard/admin-insights'
import { useGateways } from '@/lib/hooks/use-gateways'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { doctorApi, isBlockingDoctorSeverity, type DoctorReport } from '@/lib/api/doctor-client'
import { setupApi, type PublicProxyFormat, type PublicProxyRenderOutcome, type TailscaleFunnelInspection, type TailscaleFunnelMutationOutcome } from '@/lib/api/setup-client'

/**
 * Doctor panel — control-plane posture and effective defaults, restyled onto
 * the mock's settings-card vocabulary: an uppercase header bar over rows whose
 * label and description sit left of a right-aligned value.
 */
export default function SettingsPage() {
  const { data: gateways, isLoading, error } = useGateways()
  const snapshot = gateways ? buildGatewaySettingsSnapshot(gateways, {
    hasStandaloneBearerAuth: isStandaloneBearerAuthMode(),
    hasMockData: hasMockDataAuthMode(),
  }) : null

  const unavailable = Boolean(error) || !snapshot
  const [doctorReport, setDoctorReport] = useState<DoctorReport | null>(null)
  const [doctorError, setDoctorError] = useState<string>()
  const [doctorLoading, setDoctorLoading] = useState(true)
  const [publicUrl, setPublicUrl] = useState('')
  const [proxyFormat, setProxyFormat] = useState<PublicProxyFormat>('caddy')
  const [proxyAdvanced, setProxyAdvanced] = useState(false)
  const [manualProxyOpen, setManualProxyOpen] = useState(false)
  const [proxyResult, setProxyResult] = useState<PublicProxyRenderOutcome | null>(null)
  const [proxyError, setProxyError] = useState<string>()
  const [proxyLoading, setProxyLoading] = useState(false)
  const [funnel, setFunnel] = useState<TailscaleFunnelInspection | null>(null)
  const [funnelActivation, setFunnelActivation] = useState<TailscaleFunnelMutationOutcome | null>(null)
  const [funnelError, setFunnelError] = useState<string>()
  const [funnelLoading, setFunnelLoading] = useState(true)

  const runReadiness = useCallback(async () => {
    setDoctorLoading(true)
    setDoctorError(undefined)
    try {
      setDoctorReport(await doctorApi.auditFull())
    } catch (cause) {
      setDoctorError(cause instanceof Error ? cause.message : 'Doctor could not complete its readiness check.')
    } finally {
      setDoctorLoading(false)
    }
  }, [])

  const inspectFunnel = useCallback(async () => {
    setFunnelLoading(true)
    setFunnelError(undefined)
    try {
      const inspection = await setupApi.tailscaleFunnelInspect()
      setFunnel(inspection)
      if (!inspection.activation_required) setFunnelActivation(null)
      return inspection
    } catch (cause) {
      setFunnel(null)
      setFunnelError(cause instanceof Error ? cause.message : 'Labby could not inspect Tailscale Funnel.')
      return null
    } finally {
      setFunnelLoading(false)
    }
  }, [])

  useEffect(() => {
    void runReadiness()
    void inspectFunnel()
    void setupApi.settingsState('core').then((state) => {
      const configured = state.values.LABBY_PUBLIC_URL
      if (typeof configured === 'string' && configured.trim()) setPublicUrl(configured.trim())
    }).catch(() => undefined)
  }, [inspectFunnel, runReadiness])

  const blockingFindings = useMemo(
    () => doctorReport?.findings.filter((finding) => isBlockingDoctorSeverity(finding.severity)) ?? [],
    [doctorReport],
  )
  const recommendationCount = useMemo(
    () => doctorReport?.findings.filter((finding) => finding.severity === 'warn').length ?? 0,
    [doctorReport],
  )
  const readinessFinding = doctorReport?.findings.find((finding) => finding.check === 'readiness:personal')

  async function configureFunnel() {
    setFunnelLoading(true)
    setFunnelError(undefined)
    try {
      const outcome = await setupApi.tailscaleFunnelConfigure()
      if (outcome.activation_required) {
        setFunnelActivation(outcome)
        await inspectFunnel()
        return
      }
      setFunnelActivation(null)
      setPublicUrl(outcome.public_origin)
      setProxyResult(null)
      await inspectFunnel()
    } catch (cause) {
      setFunnelError(cause instanceof Error ? cause.message : 'Labby could not configure Tailscale Funnel.')
      setFunnelLoading(false)
    }
  }

  async function disableFunnel() {
    setFunnelLoading(true)
    setFunnelError(undefined)
    try {
      await setupApi.tailscaleFunnelDisable()
      setFunnelActivation(null)
      await inspectFunnel()
    } catch (cause) {
      setFunnelError(cause instanceof Error ? cause.message : 'Labby could not disable Tailscale Funnel.')
      setFunnelLoading(false)
    }
  }

  async function generatePublicProxy() {
    if (!publicUrl.trim()) return
    setProxyLoading(true)
    setProxyError(undefined)
    try {
      setProxyResult(await setupApi.publicProxyRender(publicUrl.trim(), { format: proxyFormat }))
    } catch (cause) {
      setProxyError(cause instanceof Error ? cause.message : 'Labby could not render reverse-proxy configuration.')
      setProxyResult(null)
    } finally {
      setProxyLoading(false)
    }
  }

  async function copyProxyConfig() {
    const config = proxyResult?.configs[proxyFormat]
    if (!config) return
    try {
      await navigator.clipboard.writeText(config)
      setProxyError(undefined)
    } catch {
      setProxyError('Copy failed. Select the generated configuration manually.')
    }
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
      <h2 className="sr-only">Doctor</h2>

      <SettingsCard
        title="Operational readiness"
        description="One answer for the common question: can this Labby be used right now? Warnings are recommendations; only blocking failures prevent readiness."
      >
        {doctorLoading ? (
          <LoadingRows count={3} />
        ) : doctorError ? (
          <ErrorRow message={doctorError} />
        ) : (
          <>
            <SettingsRow
              label="Status"
              description={readinessFinding?.message ?? (blockingFindings.length === 0 ? 'No blocking failures detected.' : 'Resolve the blocking findings below.')}
              control={
                <SettingsValue>
                  {blockingFindings.length === 0 ? 'Ready' : 'Needs attention'}
                </SettingsValue>
              }
            />
            <SettingsRow
              label="Blocking issues"
              description="Failures that prevent a dependable personal Labby workflow."
              control={<MetricValue value={blockingFindings.length} tone={blockingFindings.length > 0 ? 'var(--aurora-error)' : 'var(--aurora-success)'} />}
            />
            <SettingsRow
              label="Recommendations"
              description="Optional improvements or capabilities that are not required for the current setup mode."
              control={<MetricValue value={recommendationCount} tone={recommendationCount > 0 ? 'var(--aurora-warn)' : undefined} />}
            />
            <SettingsRowStrip>
              <Button variant="outline" size="sm" onClick={() => void runReadiness()}>
                <RefreshCw className="size-4" /> Run readiness check again
              </Button>
            </SettingsRowStrip>
          </>
        )}
      </SettingsCard>

      <SettingsCard
        title="Public HTTPS"
        description="Expose Labby for Browser + ChatGPT with one guided path. Tailscale Funnel is the automatic option when available; existing HTTPS and conventional reverse proxies stay available below."
      >
        <SettingsRow
          label="Recommended: Tailscale Funnel"
          description="Publishes only the local Labby backend through Tailscale-managed HTTPS. Configuration is intentionally allowed only from the local Labby control plane."
          control={
            <SettingsValue>
              {funnelLoading
                ? 'Checking…'
                : !funnel?.cli_available
                  ? 'Not detected'
                  : funnel.configured_backend === 'http://127.0.0.1:8765'
                    ? 'Configured'
                    : funnel.configured_backend
                      ? 'Port in use'
                      : funnelActivation?.activation_required || funnel.activation_required
                        ? 'Approval required'
                        : funnel.ready_to_configure
                          ? 'Ready'
                          : 'Needs attention'}
            </SettingsValue>
          }
        />
        <SettingsRowStrip>
          <div className="grid w-full gap-3 text-xs text-aurora-text-muted">
            {funnel?.public_origin ? (
              <div className="grid gap-1">
                <span><strong className="text-aurora-text-primary">Public URL:</strong> {funnel.public_origin}</span>
                {funnel.configured_backend === 'http://127.0.0.1:8765' ? (
                  <>
                    <code>{funnel.public_origin}/auth/google/callback</code>
                    <code>{funnel.public_origin}/mcp</code>
                  </>
                ) : null}
              </div>
            ) : null}
            {funnel?.configured_backend && funnel.configured_backend !== 'http://127.0.0.1:8765' ? (
              <p className="text-aurora-warn">
                Port {funnel.https_port} already belongs to {funnel.configured_backend}. Labby will not replace it.
              </p>
            ) : null}
            {funnelActivation?.activation_required ? (
              <div className="grid gap-2 rounded-aurora-2 border border-aurora-warn/35 bg-aurora-warn/5 p-3">
                <strong className="text-aurora-text-primary">One-time Tailscale approval required</strong>
                <p>Approve HTTPS + Funnel for this device, then run the same setup step again. Labby stops the waiting CLI process instead of leaving this page hung.</p>
                {funnelActivation.activation_url ? (
                  <Button
                    className="w-fit"
                    size="sm"
                    onClick={() => window.open(funnelActivation.activation_url ?? '', '_blank', 'noopener,noreferrer')}
                  >
                    <ExternalLink className="size-4" /> Open Tailscale approval
                  </Button>
                ) : funnelActivation.activation_message ? (
                  <pre className="max-h-36 overflow-auto whitespace-pre-wrap rounded-aurora-2 bg-aurora-control-surface p-2 text-[11px]">{funnelActivation.activation_message}</pre>
                ) : null}
              </div>
            ) : null}
            {funnel?.blockers.map((blocker) => <p key={blocker}>{blocker}</p>)}
            {funnelError ? <p className="text-aurora-error">{funnelError}</p> : null}
            <div className="flex flex-wrap gap-2">
              <Button variant="outline" size="sm" disabled={funnelLoading} onClick={() => void inspectFunnel()}>
                {funnelLoading ? <Loader2 className="size-4 animate-spin" /> : <RefreshCw className="size-4" />}
                Check Funnel
              </Button>
              {funnel?.ready_to_configure && !funnel.configured_backend ? (
                <Button size="sm" disabled={funnelLoading} onClick={() => void configureFunnel()}>
                  <PlugZap className="size-4" />
                  {funnelActivation?.activation_required
                    ? 'Retry after approval'
                    : funnel.activation_required
                      ? 'Start Tailscale approval'
                      : 'Use Tailscale Funnel'}
                </Button>
              ) : null}
              {funnel?.configured_backend === 'http://127.0.0.1:8765' && funnel.public_origin ? (
                <>
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => { setPublicUrl(funnel.public_origin ?? ''); setProxyResult(null) }}
                  >
                    Use this URL
                  </Button>
                  <Button variant="outline" size="sm" disabled={funnelLoading} onClick={() => void disableFunnel()}>
                    Disable Labby Funnel
                  </Button>
                </>
              ) : null}
            </div>
          </div>
        </SettingsRowStrip>
        <SettingsRowStrip>
          <button
            className="w-fit text-xs font-semibold text-aurora-accent-primary underline-offset-4 hover:underline"
            type="button"
            onClick={() => setManualProxyOpen((current) => !current)}
          >
            {manualProxyOpen ? 'Hide manual HTTPS options' : 'I already have HTTPS / use a reverse proxy'}
          </button>
        </SettingsRowStrip>
        {manualProxyOpen ? (
          <>
            <SettingsRowStrip>
              <div className="grid w-full gap-3">
                <div className="grid gap-1.5">
                  <label className="text-xs font-semibold text-aurora-text-primary" htmlFor="doctor-public-url">Public Labby URL</label>
                  <Input
                    id="doctor-public-url"
                    type="url"
                    placeholder="https://labby.example.com"
                    value={publicUrl}
                    onChange={(event) => { setPublicUrl(event.target.value); setProxyResult(null) }}
                  />
                </div>
                <button
                  className="w-fit text-xs font-semibold text-aurora-accent-primary underline-offset-4 hover:underline"
                  type="button"
                  onClick={() => {
                    setProxyAdvanced((current) => {
                      if (current) { setProxyFormat('caddy'); setProxyResult(null) }
                      return !current
                    })
                  }}
                >
                  {proxyAdvanced ? 'Use recommended Caddy config' : 'Use Nginx or Traefik instead'}
                </button>
                {proxyAdvanced ? (
                  <Select value={proxyFormat} onValueChange={(value) => { setProxyFormat(value as PublicProxyFormat); setProxyResult(null) }}>
                    <SelectTrigger><SelectValue /></SelectTrigger>
                    <SelectContent>
                      <SelectItem value="caddy">Caddy (recommended)</SelectItem>
                      <SelectItem value="nginx">Nginx</SelectItem>
                      <SelectItem value="traefik">Traefik</SelectItem>
                    </SelectContent>
                  </Select>
                ) : null}
                {proxyError ? <p className="text-xs text-aurora-error">{proxyError}</p> : null}
                <Button className="w-fit" disabled={proxyLoading || !publicUrl.trim()} onClick={() => void generatePublicProxy()}>
                  {proxyLoading ? <Loader2 className="size-4 animate-spin" /> : <PlugZap className="size-4" />}
                  {proxyLoading ? 'Generating…' : 'Generate proxy config'}
                </Button>
              </div>
            </SettingsRowStrip>
            {proxyResult?.configs[proxyFormat] ? (
              <>
                <SettingsRowStrip>
                  <div className="w-full min-w-0">
                    <div className="mb-2 flex items-center justify-between gap-3">
                      <span className="text-xs font-semibold text-aurora-text-primary">{proxyFormat.toUpperCase()} configuration</span>
                      <Button variant="outline" size="sm" onClick={() => void copyProxyConfig()}><Copy className="size-4" /> Copy</Button>
                    </div>
                    <pre className="max-h-80 overflow-auto rounded-aurora-2 border border-aurora-border-subtle bg-aurora-control-surface p-3 text-xs leading-5 text-aurora-text-muted">{proxyResult.configs[proxyFormat]}</pre>
                  </div>
                </SettingsRowStrip>
                <SettingsRowStrip>
                  <div className="grid gap-1.5 text-xs text-aurora-text-muted">
                    <strong className="text-aurora-text-primary">Public endpoints</strong>
                    <code>{proxyResult.oauth_callback_url}</code>
                    <code>{proxyResult.mcp_url}</code>
                    <strong className="mt-1 text-aurora-text-primary">Verify after reload</strong>
                    {proxyResult.verification.map((command) => <code key={command}>{command}</code>)}
                  </div>
                </SettingsRowStrip>
              </>
            ) : null}
          </>
        ) : null}
      </SettingsCard>

      <SettingsCard
        title="Fleet Posture"
        description="Control-plane posture and effective defaults for the server fleet."
      >
        {isLoading ? (
          <LoadingRows count={4} />
        ) : unavailable ? (
          <ErrorRow message="Failed to load settings because the server list is unavailable." />
        ) : (
          <>
            <SettingsRow
              label="Auth mode"
              description="How the web UI authenticates control-plane requests."
              control={<SettingsValue>{snapshot!.authModeLabel}</SettingsValue>}
            />
            <SettingsRow
              label="Runtime"
              description="Current environment mode exposed to the admin UI."
              control={<SettingsValue>{snapshot!.runtimeLabel}</SettingsValue>}
            />
            <SettingsRow
              label="Warnings"
              description="Warnings across all configured servers."
              control={
                <MetricValue
                  value={snapshot!.warningCount}
                  tone={snapshot!.warningCount > 0 ? 'var(--aurora-warn)' : undefined}
                />
              }
            />
            <SettingsRow
              label="Disconnected"
              description="Servers that currently need operator attention."
              control={
                <MetricValue
                  value={snapshot!.disconnectedGateways}
                  tone={snapshot!.disconnectedGateways > 0 ? 'var(--aurora-error)' : undefined}
                />
              }
            />
          </>
        )}
      </SettingsCard>

      <SettingsCard
        title="Control-plane posture"
        description="A read-only summary of the admin surface and the current server fleet."
      >
        {isLoading ? (
          <LoadingRows count={4} />
        ) : unavailable ? (
          <ErrorRow message="Failed to load settings because the server list is unavailable." />
        ) : (
          <>
            <SettingsRow
              label={
                <span style={{ display: 'inline-flex', alignItems: 'center', gap: 8 }}>
                  <ShieldCheck size={14} style={{ color: 'var(--aurora-accent-primary)' }} />
                  Authentication
                </span>
              }
              description={`UI requests are running in ${snapshot!.authModeLabel}.`}
            />
            <SettingsRow
              label={
                <span style={{ display: 'inline-flex', alignItems: 'center', gap: 8 }}>
                  <LifeBuoy size={14} style={{ color: 'var(--aurora-accent-primary)' }} />
                  Preview mode
                </span>
              }
              description={`${snapshot!.runtimeLabel} is active for this build.`}
            />
            <SettingsRow
              label={
                <span style={{ display: 'inline-flex', alignItems: 'center', gap: 8 }}>
                  <PlugZap size={14} style={{ color: 'var(--aurora-accent-primary)' }} />
                  Server reachability
                </span>
              }
              description={`${snapshot!.connectedGateways} of ${snapshot!.totalGateways} servers are connected.`}
            />
            <SettingsRow
              label={
                <span style={{ display: 'inline-flex', alignItems: 'center', gap: 8 }}>
                  <KeyRound size={14} style={{ color: 'var(--aurora-accent-primary)' }} />
                  Protected upstreams
                </span>
              }
              description={`${snapshot!.bearerTokenGateways} servers require bearer-token env wiring.`}
            />
          </>
        )}
      </SettingsCard>

      <SettingsCard title="Effective defaults">
        {isLoading ? (
          <LoadingRows count={3} />
        ) : unavailable ? (
          <ErrorRow message="Effective defaults are unavailable until the server list loads successfully." />
        ) : (
          <>
            <SettingsRow
              label="Proxy resources enabled"
              control={<SettingsValue>{snapshot!.proxyResourceGateways} servers</SettingsValue>}
            />
            <SettingsRow
              label="Disconnected servers"
              control={
                <MetricValue
                  value={snapshot!.disconnectedGateways}
                  tone={snapshot!.disconnectedGateways > 0 ? 'var(--aurora-error)' : undefined}
                />
              }
            />
            <SettingsRow
              label="Warning backlog"
              control={
                <MetricValue
                  value={snapshot!.warningCount}
                  tone={snapshot!.warningCount > 0 ? 'var(--aurora-warn)' : undefined}
                />
              }
            />
            <SettingsRowStrip>
              <span style={{ fontSize: 11.5, lineHeight: 1.5, color: 'var(--aurora-text-muted)' }}>
                Code Mode mode is now managed on the Servers page. Other global defaults are still
                surfaced as effective posture until their backend write APIs exist.
              </span>
            </SettingsRowStrip>
          </>
        )}
      </SettingsCard>

    </div>
  )
}

function MetricValue({ value, tone }: { value: number; tone?: string }) {
  return (
    <span
      style={{
        fontFamily: 'var(--font-display)',
        fontSize: 16,
        fontWeight: 800,
        fontVariantNumeric: 'tabular-nums',
        color: tone ?? 'var(--aurora-text-primary)',
      }}
    >
      {value}
    </span>
  )
}

function LoadingRows({ count }: { count: number }) {
  return (
    <>
      {Array.from({ length: count }, (_, index) => (
        <SettingsRowStrip key={index}>
          <span
            className="animate-pulse"
            style={{
              display: 'block',
              width: '100%',
              height: 32,
              borderRadius: 8,
              background: 'var(--aurora-control-surface)',
            }}
          />
        </SettingsRowStrip>
      ))}
    </>
  )
}

function ErrorRow({ message }: { message: string }) {
  return (
    <SettingsRowStrip>
      <span style={{ fontSize: 11.5, lineHeight: 1.5, color: 'var(--aurora-error)' }}>
        {message}
      </span>
    </SettingsRowStrip>
  )
}
