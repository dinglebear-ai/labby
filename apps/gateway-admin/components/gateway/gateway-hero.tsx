'use client'

import * as React from 'react'
import { Cable, Wrench } from 'lucide-react'

/**
 * Gateway screen hero, measured off the rendered Gateway Console mock.
 *
 * Two stat groups sit welded to the card's bottom edge — a fleet group
 * (Healthy / Enabled / Total / Disconnected) over a health bar, and an
 * exposure group (Tools / Prompts / Resources) over an exposure bar. The
 * fleet cells double as lens filters, which is how the mock boxes the active
 * one, so they carry the same `aria-pressed` contract the old summary cards did.
 *
 * Per-stat historical deltas are not reported. Host uptime is supplied by
 * the live health metadata rendered in the actions area.
 */

import type { GatewayPrimaryLens } from './gateway-list-state'

/** Lens the hero's stat cells can activate — the list's primary lenses plus the tools view. */
export type GatewayLens = GatewayPrimaryLens | 'tools'

type FleetCell = {
  label: string
  value: number
  lens?: GatewayLens
  tone?: string
}

function StatCell({
  label,
  value,
  active,
  onClick,
  tone,
}: {
  label: string
  value: React.ReactNode
  active?: boolean
  onClick?: () => void
  tone?: string
}) {
  const interactive = Boolean(onClick)
  const style: React.CSSProperties = {
    flex: '1 1 0%',
    minWidth: 0,
    textAlign: 'center',
    fontFamily: 'inherit',
    padding: '6px 4px',
    borderRadius: 10,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: active
      ? 'color-mix(in srgb, var(--aurora-accent-primary) 40%, transparent)'
      : 'transparent',
    background: active
      ? 'color-mix(in srgb, var(--aurora-accent-primary) 9%, transparent)'
      : 'none',
    transition: 'background 150ms, border-color 150ms',
    cursor: interactive ? 'pointer' : 'default',
  }

  const body = (
    <>
      <div
        style={{
          fontFamily: 'var(--font-display)',
          fontSize: 24,
          lineHeight: 1,
          fontWeight: 800,
          fontVariantNumeric: 'tabular-nums',
          color: tone ?? 'var(--aurora-text-primary)',
        }}
      >
        {value}
      </div>
      <div
        style={{
          marginTop: 5,
          fontSize: 9.5,
          fontWeight: 700,
          letterSpacing: '0.08em',
          textTransform: 'uppercase',
          color: 'var(--aurora-text-muted)',
          whiteSpace: 'nowrap',
          overflow: 'hidden',
          textOverflow: 'ellipsis',
        }}
      >
        {label}
      </div>
    </>
  )

  if (!interactive) {
    return (
      <div data-gateway-stat={label.toLowerCase()} style={style}>
        {body}
      </div>
    )
  }
  const accessibleValue =
    typeof value === 'string' || typeof value === 'number' ? `${label}: ${value}` : label
  return (
    <button
      type="button"
      data-gateway-stat={label.toLowerCase()}
      onClick={onClick}
      aria-label={accessibleValue}
      aria-pressed={Boolean(active)}
      style={style}
    >
      {body}
    </button>
  )
}

function GroupIcon({ label, icon }: { label: string; icon: React.ReactNode }) {
  return (
    <div
      style={{
        flexShrink: 0,
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        gap: 6,
        padding: '0 14px 0 12px',
        borderRight:
          '1px solid color-mix(in srgb, var(--aurora-border-default) 55%, var(--aurora-page-bg))',
      }}
    >
      <span
        style={{
          display: 'grid',
          placeItems: 'center',
          width: 30,
          height: 30,
          borderRadius: 9,
          border:
            '1px solid color-mix(in srgb, var(--aurora-border-default) 70%, var(--aurora-page-bg))',
          background: 'color-mix(in srgb, var(--aurora-control-surface) 75%, transparent)',
          color: 'var(--aurora-accent-strong)',
        }}
      >
        {icon}
      </span>
      <span
        style={{
          fontSize: 10,
          fontWeight: 700,
          letterSpacing: '0.13em',
          textTransform: 'uppercase',
          color: 'var(--aurora-text-muted)',
          whiteSpace: 'nowrap',
        }}
      >
        {label}
      </span>
    </div>
  )
}

/** Segmented bar — one slice per server, coloured by state. */
function HealthBar({ segments }: { segments: { key: string; color: string; title: string }[] }) {
  return (
    <span
      style={{
        display: 'flex',
        gap: 2,
        height: 4,
        marginTop: 8,
        borderRadius: 999,
        overflow: 'hidden',
      }}
    >
      {segments.map((segment) => (
        <span
          key={segment.key}
          title={segment.title}
          style={{ flex: 1, background: segment.color, borderRadius: 999 }}
        />
      ))}
    </span>
  )
}

export function GatewayHero({
  totalServers,
  healthy,
  enabled,
  disconnected,
  discoveredTools,
  exposedTools,
  discoveredPrompts,
  exposedPrompts,
  discoveredResources,
  exposedResources,
  discoveredSkills,
  exposedSkills,
  serverStates,
  endpointLabel,
  activeLens,
  toolsViewActive,
  onLensChange,
  actions,
}: {
  totalServers: number
  healthy: number
  enabled: number
  disconnected: number
  discoveredTools: number
  exposedTools: number
  discoveredPrompts: number
  exposedPrompts: number
  discoveredResources: number
  exposedResources: number
  discoveredSkills: number
  exposedSkills: number
  /** One entry per server, for the segmented health bar. */
  serverStates: { id: string; name: string; color: string; state: string }[]
  endpointLabel?: string
  activeLens: GatewayLens
  toolsViewActive: boolean
  onLensChange: (lens: GatewayLens) => void
  actions?: React.ReactNode
}) {
  const attention = serverStates.filter((server) => ['disconnected', 'needs attention'].includes(server.state)).length
  const discovering = serverStates.filter((server) => server.state === 'discovering').length
  const pulseColor =
    attention > 0
      ? 'var(--aurora-warn)'
      : discovering > 0
        ? 'var(--aurora-accent-strong)'
        : totalServers > 0
          ? 'var(--aurora-success)'
          : 'var(--aurora-text-muted)'
  const pulseLabel =
    totalServers === 0
      ? 'no servers'
      : attention > 0
        ? `${attention} need${attention === 1 ? 's' : ''} attention`
        : discovering > 0
          ? `${discovering} discovering`
          : 'all systems nominal'

  const totalDiscovered =
    discoveredTools + discoveredPrompts + discoveredResources + discoveredSkills
  const totalExposed = exposedTools + exposedPrompts + exposedResources + exposedSkills
  const exposedPct =
    totalDiscovered === 0 ? 0 : Math.round((totalExposed / totalDiscovered) * 100)

  const fleetCells: FleetCell[] = [
    { label: 'Healthy', value: healthy, lens: 'healthy' },
    { label: 'Enabled', value: enabled, lens: 'enabled' },
    { label: 'Total', value: totalServers, lens: 'configured' },
    {
      label: 'Disconnected',
      value: disconnected,
      lens: 'disconnected',
      tone: disconnected > 0 ? 'var(--aurora-warn)' : undefined,
    },
  ]

  return (
    <div
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
        style={{
          display: 'flex',
          alignItems: 'flex-end',
          justifyContent: 'space-between',
          gap: 16,
          padding: '20px 24px 15px',
          flexWrap: 'wrap',
        }}
      >
        <div style={{ minWidth: 0 }}>
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
            <button type="button" onClick={() => onLensChange('attention')} aria-label={attention > 0 ? `Needs attention: ${attention}` : `Gateway status: ${pulseLabel}`} aria-pressed={!toolsViewActive && activeLens === 'attention'} style={{ display: 'inline-flex', alignItems: 'center', gap: 6, border: 'none', background: activeLens === 'attention' ? 'var(--aurora-hover-bg)' : 'none', padding: '2px 7px', margin: '-2px -7px', borderRadius: 7, cursor: 'pointer' }}>
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
            </button>
          </div>
          <h1
            style={{
              margin: '8px 0 0',
              fontFamily: 'var(--font-display)',
              fontSize: 30,
              lineHeight: 1.04,
              fontWeight: 800,
              color: 'var(--aurora-text-primary)',
              whiteSpace: 'nowrap',
            }}
          >
            Gateway
          </h1>
        </div>

        {actions ? (
          <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>{actions}</div>
        ) : endpointLabel ? (
          <span
            style={{
              fontSize: 11.5,
              color: 'var(--aurora-text-muted)',
              fontFamily: 'var(--font-mono)',
              whiteSpace: 'nowrap',
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              maxWidth: '40%',
            }}
          >
            {endpointLabel}
          </span>
        ) : null}
      </div>

      <div
        style={{
          borderTop:
            '1px solid color-mix(in srgb, var(--aurora-border-default) 55%, var(--aurora-page-bg))',
          background: 'var(--gw0-0_28)',
          padding: '12px 14px',
          borderRadius: '0 0 var(--radius-3) var(--radius-3)',
          display: 'grid',
          gridTemplateColumns: 'repeat(auto-fit, minmax(min(100%, 320px), 1fr))',
          gap: '10px 28px',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'stretch', padding: 0 }}>
          <GroupIcon label="Servers" icon={<Cable size={15} strokeWidth={1.7} />} />
          <div style={{ flex: '1 1 0%', minWidth: 0, display: 'flex', flexDirection: 'column' }}>
            <div style={{ display: 'flex' }}>
              {fleetCells.map((cell) => (
                <StatCell
                  key={cell.label}
                  label={cell.label}
                  value={cell.value}
                  tone={cell.tone}
                  active={
                    cell.lens ? !toolsViewActive && activeLens === cell.lens : undefined
                  }
                  onClick={cell.lens ? () => onLensChange(cell.lens as GatewayLens) : undefined}
                />
              ))}
            </div>
            <HealthBar
              segments={serverStates.map((server) => ({
                key: server.id,
                color: server.color,
                title: `${server.name} — ${server.state}`,
              }))}
            />
          </div>
        </div>

        <div style={{ display: 'flex', alignItems: 'stretch', padding: 0 }}>
          <GroupIcon label="Exposure" icon={<Wrench size={15} strokeWidth={1.7} />} />
          <div style={{ flex: '1 1 0%', minWidth: 0, display: 'flex', flexDirection: 'column' }}>
            <div style={{ display: 'flex' }}>
              <StatCell
                label="Tools"
                tone="var(--aurora-accent-pink)"
                value={`${exposedTools}/${discoveredTools}`}
                active={toolsViewActive}
                onClick={() => onLensChange('tools')}
              />
              <StatCell
                label="Prompts"
                tone="var(--aurora-accent-strong)"
                value={`${exposedPrompts}/${discoveredPrompts}`}
              />
              <StatCell
                label="Resources"
                tone="var(--aurora-accent-strong)"
                value={`${exposedResources}/${discoveredResources}`}
              />

            </div>
            <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginTop: 8 }}>
              <span
                style={{
                  flex: 1,
                  minWidth: 0,
                  height: 4,
                  borderRadius: 999,
                  background: 'var(--aurora-control-surface)',
                  overflow: 'hidden',
                }}
              >
                <span
                  style={{
                    display: 'block',
                    height: '100%',
                    width: `${exposedPct}%`,
                    borderRadius: 999,
                    background: 'var(--aurora-accent-primary)',
                  }}
                />
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
                <span title={`Including ${exposedSkills}/${discoveredSkills} skills`}>{exposedPct}% exposed</span>
              </span>
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}
