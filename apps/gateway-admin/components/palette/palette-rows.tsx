'use client'

/**
 * Palette row variants, matched against the mock's `[data-palrow]` markup.
 *
 * Rows are `CommandItem`s so cmdk keeps owning arrow-key navigation and Enter
 * activation; the mock's own cursor styling is reproduced through
 * `[data-palrow][data-selected='true']` in `palette-styles.tsx`.
 */

import type { MouseEvent, ReactNode } from 'react'
import { Check, Copy, Play, Power, RefreshCw } from 'lucide-react'

import { CommandItem } from '@/components/ui/command'
import type { GatewayConnection, PaletteTone } from '@/lib/app-command-palette'
import type { Gateway } from '@/lib/types/gateway'

import { PaletteChip, PaletteDot, paletteToneVar } from './palette-parts'

function stop(event: MouseEvent) {
  event.stopPropagation()
  event.preventDefault()
}

/** Generic action / page / service row: icon chip, label, uppercase trailing. */
export function PaletteCommandRow({
  value,
  keywords,
  zebra,
  icon,
  iconTone,
  label,
  trailing,
  trailingPlain,
  onSelect,
}: {
  value: string
  keywords?: string[]
  zebra: number
  icon: ReactNode
  iconTone?: 'accent' | 'muted'
  label: string
  trailing: string
  trailingPlain?: boolean
  onSelect: () => void
}) {
  return (
    <CommandItem
      data-palrow="1"
      data-palzebra={zebra % 2 === 1 ? '1' : '0'}
      value={value}
      keywords={keywords}
      onSelect={onSelect}
    >
      <PaletteChip tone={iconTone}>{icon}</PaletteChip>
      <span className="pal-label">{label}</span>
      <span className="pal-grow" />
      <span className="pal-sub" data-plain={trailingPlain ? '1' : undefined}>
        {trailing}
      </span>
    </CommandItem>
  )
}

/** "Needs Attention" row — status dot plus a tone-coloured `Actions ›` affordance. */
export function PaletteAlertRow({
  value,
  label,
  tone,
  onSelect,
}: {
  value: string
  label: string
  tone: PaletteTone
  onSelect: () => void
}) {
  return (
    <CommandItem data-palrow="1" data-alerttone={tone} value={value} onSelect={onSelect}>
      <PaletteChip tone="bare">
        <PaletteDot tone={tone} />
      </PaletteChip>
      <span className="pal-label">{label}</span>
      <span className="pal-grow" />
      <span
        className="pal-sub"
        data-plain="1"
        style={{ color: paletteToneVar(tone), fontWeight: 650 }}
      >
        Actions ›
      </span>
    </CommandItem>
  )
}

/** Server row — dot, name, endpoint, hover-revealed controls, transport label. */
export function PaletteServerRow({
  gateway,
  endpoint,
  connection,
  zebra,
  copied,
  pending,
  onSelect,
  onCopy,
  onTogglePower,
  onTest,
  onReload,
}: {
  gateway: Gateway
  endpoint: string
  connection: GatewayConnection
  zebra: number
  copied: boolean
  pending: boolean
  onSelect: () => void
  onCopy: () => void
  onTogglePower: () => void
  onTest: () => void
  onReload: () => void
}) {
  const enabled = gateway.enabled ?? true
  return (
    <CommandItem
      data-palrow="1"
      data-hoverrow="1"
      data-palzebra={zebra % 2 === 1 ? '1' : '0'}
      value={`gateway:${gateway.id}`}
      keywords={[gateway.name, endpoint, gateway.transport]}
      onSelect={onSelect}
    >
      <PaletteDot tone={connection.tone} variant="halo" />
      <span className="pal-name">{gateway.name}</span>
      <span className="pal-grow inline-flex items-center gap-[5px]">
        <span className="pal-endpoint">{endpoint}</span>
        <button
          type="button"
          className="pal-rowbtn"
          data-small="1"
          data-hoverreveal="1"
          aria-label={`Copy connection JSON for ${gateway.name}`}
          title={`Copy .mcp.json entry for ${gateway.name}`}
          disabled={pending}
          onClick={(event) => {
            stop(event)
            onCopy()
          }}
        >
          {copied ? <Check size={11} /> : <Copy size={11} />}
        </button>
      </span>
      <span className="inline-flex flex-shrink-0 items-center gap-[3px]">
        <button
          type="button"
          className="pal-rowbtn"
          data-hoverreveal="1"
          aria-label={enabled ? `Disable ${gateway.name}` : `Enable ${gateway.name}`}
          title={enabled ? 'Disable server' : 'Enable server'}
          disabled={pending}
          onClick={(event) => {
            stop(event)
            onTogglePower()
          }}
        >
          <Power size={12} />
        </button>
        <button
          type="button"
          className="pal-rowbtn"
          data-hoverreveal="1"
          aria-label={`Test connection for ${gateway.name}`}
          title="Test connection"
          disabled={pending}
          onClick={(event) => {
            stop(event)
            onTest()
          }}
        >
          <Play size={12} />
        </button>
        <button
          type="button"
          className="pal-rowbtn"
          data-hoverreveal="1"
          aria-label={`Reload ${gateway.name}`}
          title="Reload server"
          disabled={pending}
          onClick={(event) => {
            stop(event)
            onReload()
          }}
        >
          <RefreshCw size={12} />
        </button>
      </span>
      <span className="pal-transport">{gateway.transport}</span>
    </CommandItem>
  )
}
