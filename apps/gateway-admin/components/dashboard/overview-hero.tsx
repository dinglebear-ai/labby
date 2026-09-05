'use client'

import * as React from 'react'
import Link from 'next/link'
import {
  Activity,
  AlertTriangle,
  Cable,
  Coins,
  FileText,
  BookOpen,
  Gauge,
  type LucideIcon,
  MessageSquare,
  PlugZap,
  RotateCw,
  Wrench,
} from 'lucide-react'

import { formatCompactNumber } from '@/lib/dashboard/dashboard-metrics'
import type { LiveFleetStats } from '@/lib/dashboard/dashboard-metrics'
import {
  METRICS_WINDOWS,
  type DashboardMetrics,
  type MetricsWindow,
} from '@/lib/types/metrics'
import type { Gateway } from '@/lib/types/gateway'

/**
 * The Overview hero, matching the Gateway Console mock: an eyebrow with a live
 * pulse chip, the display title paired with a 24h heartbeat sparkline, trouble
 * chips for servers needing attention, and — welded to the card's bottom edge —
 * the stat strip and the per-server fleet-health squares.
 */

type Tone = 'default' | 'success' | 'warning' | 'error' | 'info'

const TONE_COLOR: Record<Tone, string> = {
  default: 'var(--aurora-text-primary)',
  success: 'var(--aurora-success)',
  warning: 'var(--aurora-warn)',
  error: 'var(--aurora-error)',
  info: 'var(--aurora-accent-strong)',
}

type HeroStat = {
  label: string
  value: string | number
  icon: LucideIcon
  tone?: Tone
  href?: string
}

/**
 * Rolling-window pills. The hero uses its own pill shape rather than the
 * shared `WindowSelector` because the mock renders them as free-standing
 * 999px pills with a solid accent fill when active, not as a bordered group.
 */
function HeroWindowPills({
  value,
  onChange,
}: {
  value: MetricsWindow
  onChange: (window: MetricsWindow) => void
}) {
  return (
    <div role="tablist" aria-label="Activity window" style={{ display: 'inline-flex', gap: 5 }}>
      {METRICS_WINDOWS.map((window) => {
        const active = window === value
        return (
          <button
            key={window}
            type="button"
            role="tab"
            aria-selected={active}
            onClick={() => onChange(window)}
            style={{
              height: 28,
              padding: '0 13px',
              borderRadius: 999,
              fontFamily: 'inherit',
              fontSize: 11.5,
              fontWeight: 650,
              cursor: 'pointer',
              whiteSpace: 'nowrap',
              borderWidth: 1,
              borderStyle: 'solid',
              borderColor: active
                ? 'color-mix(in srgb, var(--aurora-accent-primary) 70%, #0b2233)'
                : 'color-mix(in srgb, var(--aurora-border-strong) 70%, var(--aurora-page-bg))',
              background: active ? 'var(--aurora-accent-primary)' : 'transparent',
              color: active ? 'rgb(6, 32, 46)' : 'var(--aurora-text-muted)',
              boxShadow: active
                ? '0 0 0 1px color-mix(in srgb, var(--aurora-accent-primary) 30%, transparent), inset 0 1px 0 rgba(255,255,255,0.25)'
                : undefined,
              transition:
                'background 150ms ease-out, color 150ms ease-out, border-color 150ms ease-out, box-shadow 150ms ease-out',
            }}
          >
            {window}
          </button>
        )
      })}
    </div>
  )
}

/** Live "updated Ns ago" ticker — the mock's refresh affordance counts up. */
function useSecondsSince(stamp: number): number {
  const [, force] = React.useReducer((n: number) => n + 1, 0)
  React.useEffect(() => {
    const id = setInterval(force, 1000)
    return () => clearInterval(id)
  }, [])
  return Math.max(0, Math.round((Date.now() - stamp) / 1000))
}

function formatAgo(seconds: number): string {
  if (seconds < 60) return `updated ${seconds}s ago`
  if (seconds < 3600) return `updated ${Math.floor(seconds / 60)}m ago`
  return `updated ${Math.floor(seconds / 3600)}h ago`
}

/** Normalises buckets into a 0–26 polyline over a 0–100 viewBox. */
function heartbeatPoints(buckets: { calls: number }[]): string {
  if (buckets.length === 0) return '0,13 100,13'
  const peak = Math.max(...buckets.map((bucket) => bucket.calls), 1)
  const step = buckets.length > 1 ? 100 / (buckets.length - 1) : 100
  return buckets
    .map((bucket, index) => {
      const x = (index * step).toFixed(2)
      // 2px padding top and bottom keeps the stroke inside the box.
      const y = (24 - (bucket.calls / peak) * 22).toFixed(2)
      return `${x},${y}`
    })
    .join(' ')
}

function gatewayTone(gateway: Gateway): { color: string; state: string } {
  if (!gateway.status.connected) {
    return { color: 'var(--aurora-error)', state: 'disconnected' }
  }
  if (!gateway.status.healthy) {
    return { color: 'var(--aurora-warn)', state: 'unhealthy' }
  }
  if (gateway.warnings.length > 0) {
    return { color: 'var(--aurora-warn)', state: `${gateway.warnings.length} warning(s)` }
  }
  return { color: 'var(--aurora-success)', state: 'healthy' }
}

function StatCell({ stat, isLast }: { stat: HeroStat; isLast: boolean }) {
  const Icon = stat.icon
  const style: React.CSSProperties = {
    minWidth: 0,
    padding: '4px 12px',
    borderRadius: 8,
    textDecoration: 'none',
    borderRight: isLast
      ? undefined
      : '1px solid color-mix(in srgb, var(--aurora-border-default) 45%, var(--aurora-page-bg))',
  }
  const content = (
    <>
      <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
        <span style={{ flexShrink: 0, color: 'var(--aurora-text-muted)', display: 'grid' }}><Icon size={12} strokeWidth={1.8} /></span>
        <span style={{ fontSize: 9.5, fontWeight: 700, letterSpacing: '0.08em', textTransform: 'uppercase', color: 'var(--aurora-text-muted)', whiteSpace: 'nowrap' }}>{stat.label}</span>
      </div>
      <div style={{ marginTop: 6, fontFamily: 'var(--font-display)', fontSize: 21, lineHeight: 1, fontWeight: 800, fontVariantNumeric: 'tabular-nums', color: TONE_COLOR[stat.tone ?? 'default'] }}>{stat.value}</div>
    </>
  )
  if (stat.href) {
    return <Link href={stat.href} title={`${stat.label} — open details`} style={style} className="transition-colors hover:bg-aurora-hover-bg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary/40">{content}</Link>
  }
  return <div title={stat.label} style={style}>{content}</div>
}

export function OverviewHero({
  gateways,
  live,
  metrics,
  activeWindow,
  onWindowChange,
  onRefresh,
  loadedAt,
}: {
  gateways: Gateway[]
  live: LiveFleetStats
  metrics: DashboardMetrics | undefined
  activeWindow: MetricsWindow
  onWindowChange: (window: MetricsWindow) => void
  onRefresh: () => void
  /** Epoch ms of the last successful metrics load, for the "updated Ns ago" ticker. */
  loadedAt: number
}) {
  const [refreshHovered, setRefreshHovered] = React.useState(false)
  const [manageHovered, setManageHovered] = React.useState(false)
  const secondsSinceLoad = useSecondsSince(loadedAt)

  const troubled = gateways.filter(
    (gateway) =>
      !gateway.status.connected || !gateway.status.healthy || gateway.warnings.length > 0,
  )
  const allHealthy = troubled.length === 0 && gateways.length > 0
  const pulseColor = allHealthy
    ? 'var(--aurora-success)'
    : troubled.length > 0
      ? 'var(--aurora-warn)'
      : 'var(--aurora-text-muted)'
  const pulseLabel = gateways.length === 0
    ? 'no servers'
    : allHealthy
      ? 'all systems nominal'
      : `${troubled.length} need${troubled.length === 1 ? 's' : ''} attention`

  const discoveredPrompts = gateways.reduce(
    (sum, gateway) => sum + gateway.status.discovered_prompt_count,
    0,
  )
  const discoveredResources = gateways.reduce(
    (sum, gateway) => sum + gateway.status.discovered_resource_count,
    0,
  )
  const discoveredSkills = gateways.reduce(
    (sum, gateway) => sum + (gateway.status.discovered_skill_count ?? 0),
    0,
  )

  // The mock's strip vocabulary, extended with Skills as a first-class MCP
  // primitive: Connected · Offline · Tools · Prompts · Resources · Skills ·
  // Upstream calls · Failed · Tokens · P95 latency. Only Failed carries a tone;
  // every other value renders in primary text.
  const usageHref = `/usage/?window=${activeWindow}`
  const stats: HeroStat[] = [
    { label: 'Connected', value: live.connectedServers, icon: Cable, href: '/gateways/' },
    { label: 'Offline', value: live.offlineServers, icon: PlugZap, href: '/gateways/' },
    { label: 'Tools', value: live.discoveredTools, icon: Wrench, href: '/tools/' },
    { label: 'Prompts', value: discoveredPrompts, icon: MessageSquare, href: '/gateways/' },
    { label: 'Resources', value: discoveredResources, icon: FileText, href: '/gateways/' },
    { label: 'Skills', value: discoveredSkills, icon: BookOpen, href: '/skills/' },
    {
      label: 'Upstream calls',
      value: metrics ? formatCompactNumber(metrics.tool_calls.total) : '—',
      icon: Activity,
      href: usageHref,
    },
    {
      label: 'Failed',
      value: metrics ? formatCompactNumber(metrics.tool_calls.failed) : '—',
      icon: AlertTriangle,
      tone: metrics && metrics.tool_calls.failed > 0 ? 'error' : 'default',
      href: `${usageHref}&outcome=failed`,
    },
    {
      label: 'Tokens',
      value: metrics ? formatCompactNumber(metrics.tokens.total) : '—',
      icon: Coins,
      href: `${usageHref}&focus=tokens`,
    },
    {
      label: 'P95 latency',
      value: metrics ? `${Math.round(metrics.latency.p95)}ms` : '—',
      icon: Gauge,
      href: `${usageHref}&focus=latency`,
    },
  ]

  return (
    <div
      data-console-hero="1"
      style={{
        borderRadius: 'var(--radius-3)',
        border:
          '1px solid color-mix(in srgb, var(--aurora-border-default) 45%, var(--aurora-page-bg))',
        background:
          'linear-gradient(180deg, var(--aurora-panel-strong-top), var(--aurora-panel-strong))',
        boxShadow: 'var(--aurora-shadow-strong), inset 0 1px 0 rgba(255,255,255,0.05)',
      }}
    >
      <div
        data-console-hero-main="1"
        style={{
          display: 'flex',
          alignItems: 'flex-end',
          justifyContent: 'space-between',
          gap: 16,
          padding: '22px 24px 18px',
          flexWrap: 'wrap',
        }}
      >
        <div data-console-hero-copy="1" style={{ minWidth: 0 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 10, flexWrap: 'wrap' }}>
            <span
              style={{
                fontSize: 10.5,
                fontWeight: 700,
                letterSpacing: '0.16em',
                textTransform: 'uppercase',
                color: 'var(--aurora-text-muted)',
              }}
            >
              Gateway Control Plane
            </span>
            <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}>
              <span
                style={{
                  width: 6,
                  height: 6,
                  borderRadius: 999,
                  background: pulseColor,
                  boxShadow: `0 0 4px ${pulseColor}`,
                  animation: 'ovPulse 2.4s ease-in-out infinite',
                }}
              />
              <span style={{ fontSize: 10.5, fontWeight: 650, color: pulseColor }}>
                {pulseLabel}
              </span>
            </span>
          </div>

          <div
            style={{
              marginTop: 8,
              display: 'flex',
              alignItems: 'center',
              gap: 14,
              minWidth: 0,
              flexWrap: 'wrap',
            }}
          >
            <h1
              data-console-hero-title="1"
              style={{
                margin: 0,
                fontFamily: 'var(--font-display)',
                fontSize: 30,
                lineHeight: 1.04,
                fontWeight: 800,
                color: 'var(--aurora-text-primary)',
                whiteSpace: 'nowrap',
              }}
            >
              Operational Overview
            </h1>
            {metrics ? (
              <svg
                style={{ flexShrink: 0, width: 92, height: 26, opacity: 0.9 }}
                viewBox="0 0 100 26"
                preserveAspectRatio="none"
                aria-label="Fleet call volume for the selected window"
              >
                <polyline
                  points={heartbeatPoints(metrics.timeseries)}
                  fill="none"
                  stroke="var(--aurora-accent-primary)"
                  strokeWidth="1.4"
                  vectorEffect="non-scaling-stroke"
                  strokeLinejoin="round"
                  strokeLinecap="round"
                />
              </svg>
            ) : null}
          </div>

          {troubled.length > 0 ? (
            <div
              style={{
                marginTop: 9,
                display: 'flex',
                alignItems: 'center',
                gap: 6,
                flexWrap: 'wrap',
              }}
            >
              {troubled.slice(0, 4).map((gateway) => {
                const tone = gatewayTone(gateway)
                return (
                  <Link
                    key={gateway.id}
                    href="/gateways"
                    title={`${gateway.name} — ${tone.state}`}
                    style={{
                      display: 'inline-flex',
                      alignItems: 'center',
                      gap: 5,
                      height: 22,
                      padding: '0 9px',
                      borderRadius: 7,
                      border: `1px solid color-mix(in srgb, ${tone.color} 34%, transparent)`,
                      background: `color-mix(in srgb, ${tone.color} 10%, transparent)`,
                      color: tone.color,
                      fontFamily: 'inherit',
                      fontSize: 10.5,
                      fontWeight: 650,
                      whiteSpace: 'nowrap',
                      textDecoration: 'none',
                    }}
                  >
                    <span
                      style={{
                        width: 5,
                        height: 5,
                        borderRadius: 999,
                        background: 'currentColor',
                      }}
                    />
                    {gateway.name}
                  </Link>
                )
              })}
            </div>
          ) : null}
        </div>

        <div data-console-hero-actions="1" style={{ flexShrink: 0, display: 'flex', alignItems: 'center', gap: 6 }}>
          <button
            type="button"
            data-icon-text-control="1"
            onClick={onRefresh}
            title="Refresh"
            aria-label="Refresh"
            onMouseEnter={() => setRefreshHovered(true)}
            onMouseLeave={() => setRefreshHovered(false)}
            style={{
              display: 'inline-flex',
              alignItems: 'center',
              gap: 6,
              height: 28,
              padding: '0 9px',
              borderRadius: 8,
              border: 'none',
              background: refreshHovered ? 'var(--aurora-hover-bg)' : 'none',
              color: refreshHovered ? 'var(--aurora-text-primary)' : 'var(--aurora-text-muted)',
              fontFamily: 'inherit',
              fontSize: 11,
              fontVariantNumeric: 'tabular-nums',
              cursor: 'pointer',
              whiteSpace: 'nowrap',
            }}
          >
            <RotateCw size={12} strokeWidth={1.7} />
            {formatAgo(secondsSinceLoad)}
          </button>

          <span
            style={{
              width: 1,
              height: 18,
              margin: '0 4px',
              background: 'var(--aurora-border-default)',
            }}
          />

          <HeroWindowPills value={activeWindow} onChange={onWindowChange} />

          <span
            style={{
              width: 1,
              height: 18,
              margin: '0 4px',
              background: 'var(--aurora-border-default)',
            }}
          />

          <Link
            href="/gateways"
            onMouseEnter={() => setManageHovered(true)}
            onMouseLeave={() => setManageHovered(false)}
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 7,
              height: 36,
              padding: '0 16px',
              whiteSpace: 'nowrap',
              borderRadius: 10,
              border:
                '1px solid color-mix(in srgb, var(--aurora-accent-primary) 55%, var(--aurora-border-strong))',
              background: manageHovered
                ? 'color-mix(in srgb, var(--aurora-accent-primary) 13%, var(--aurora-panel-strong))'
                : 'color-mix(in srgb, var(--aurora-accent-primary) 9%, var(--aurora-panel-strong))',
              color: '#bfe7fb',
              fontFamily: 'inherit',
              fontSize: 13,
              fontWeight: 650,
              textDecoration: 'none',
              boxShadow: manageHovered
                ? '0 0 0 1px color-mix(in srgb, var(--aurora-accent-primary) 34%, transparent), inset 0 1px 0 rgba(255,255,255,0.07)'
                : 'inset 0 1px 0 rgba(255,255,255,0.05)',
              transition: 'all 150ms ease-out',
            }}
          >
            Manage Servers
          </Link>
        </div>
      </div>

      {/* Stat strip welded to the card's bottom edge */}
      <div
        style={{
          padding: '12px 14px',
          borderTop:
            '1px solid color-mix(in srgb, var(--aurora-border-default) 55%, var(--aurora-page-bg))',
          background: 'var(--gw0-0_28)',
          borderRadius: '0 0 var(--radius-3) var(--radius-3)',
          display: 'flex',
          flexDirection: 'column',
          gap: 10,
        }}
      >
        <div
          data-mobile-grid2="1"
          style={{
            display: 'grid',
            gridTemplateColumns: 'repeat(auto-fit, minmax(118px, 1fr))',
            gap: '8px 0',
          }}
        >
          {stats.map((stat, index) => (
            <StatCell key={stat.label} stat={stat} isLast={index === stats.length - 1} />
          ))}
        </div>

        <Link
          href="/gateways"
          title="Fleet health — one square per server. Click to open Gateway."
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 12,
            padding: '8px 12px 2px',
            borderTop:
              '1px solid color-mix(in srgb, var(--aurora-border-default) 40%, var(--aurora-page-bg))',
            textAlign: 'left',
            fontFamily: 'inherit',
            minWidth: 0,
            textDecoration: 'none',
          }}
        >
          <span
            style={{
              flexShrink: 0,
              fontSize: 9.5,
              fontWeight: 700,
              letterSpacing: '0.11em',
              textTransform: 'uppercase',
              color: 'var(--aurora-text-muted)',
              whiteSpace: 'nowrap',
            }}
          >
            Fleet Health
          </span>
          <span
            style={{
              flex: 1,
              minWidth: 0,
              display: 'grid',
              gridTemplateColumns: 'repeat(auto-fill, minmax(11px, 1fr))',
              gap: 3,
            }}
          >
            {gateways.map((gateway) => {
              const tone = gatewayTone(gateway)
              return (
                <span
                  key={gateway.id}
                  title={`${gateway.name} — ${tone.state}`}
                  style={{
                    display: 'block',
                    aspectRatio: '1',
                    borderRadius: 2.5,
                    background: tone.color,
                  }}
                />
              )
            })}
          </span>
          <span
            style={{
              flexShrink: 0,
              fontSize: 10,
              color: 'var(--aurora-text-muted)',
              fontVariantNumeric: 'tabular-nums',
              whiteSpace: 'nowrap',
            }}
          >
            {live.connectedServers}/{live.totalServers} healthy
          </span>
        </Link>
      </div>
    </div>
  )
}
