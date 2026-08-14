'use client'

import { KeyRound, LifeBuoy, PlugZap, ShieldCheck } from 'lucide-react'
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
import { useBrowserSession } from '@/lib/auth/session'
import { AllowedUsersPanel } from '@/components/allowed-users-panel'

/**
 * Doctor panel — control-plane posture and effective defaults, restyled onto
 * the mock's settings-card vocabulary: an uppercase header bar over rows whose
 * label and description sit left of a right-aligned value.
 */
export default function SettingsPage() {
  const session = useBrowserSession()
  const isAdmin = session.status === 'authenticated' && session.isAdmin === true
  const { data: gateways, isLoading, error } = useGateways()
  const snapshot = gateways ? buildGatewaySettingsSnapshot(gateways, {
    hasStandaloneBearerAuth: isStandaloneBearerAuthMode(),
    hasMockData: hasMockDataAuthMode(),
  }) : null

  const unavailable = Boolean(error) || !snapshot

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
      <h2 className="sr-only">Doctor</h2>

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

      {/* Allowed users (admin only) */}
      {isAdmin ? <AllowedUsersPanel /> : null}
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
