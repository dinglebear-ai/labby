'use client'

import { Check, ChevronDown } from 'lucide-react'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'

export type OverviewChartMode = 'servers' | 'volume' | 'errors' | 'outcomes'
const CHARTS: Array<{ value: OverviewChartMode; label: string }> = [
  { value: 'servers', label: 'Calls by server' },
  { value: 'volume', label: 'Call volume' },
  { value: 'errors', label: 'Success vs errors' },
  { value: 'outcomes', label: 'Outcome mix' },
]

export function OverviewChartMenu({ value, onChange }: { value: OverviewChartMode; onChange: (value: OverviewChartMode) => void }) {
  return <DropdownMenu><DropdownMenuTrigger data-visible-label aria-label="Overview chart" title="Switch chart" className="-mx-1.5 -my-0.5 inline-flex items-center gap-1.5 rounded-[7px] border-0 bg-transparent px-1.5 py-0.5 text-[10.5px] font-bold uppercase tracking-[.15em] text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary">{CHARTS.find(chart => chart.value === value)?.label}<ChevronDown size={11} /></DropdownMenuTrigger><DropdownMenuContent align="start" sideOffset={6} className="w-56 rounded-xl p-1" style={{ background: 'linear-gradient(180deg, var(--aurora-panel-strong-top), var(--aurora-panel-strong))', boxShadow: 'var(--aurora-shadow-strong), inset 0 1px 0 rgba(255,255,255,0.05)' }}>{CHARTS.map(chart => <DropdownMenuItem key={chart.value} onSelect={() => onChange(chart.value)} className="h-[30px] rounded-lg px-[9px] text-xs font-medium" style={value === chart.value ? { background: 'color-mix(in srgb, var(--aurora-accent-primary) 10%, transparent)', color: 'var(--aurora-accent-strong)' } : undefined}><span className="flex-1">{chart.label}</span>{value === chart.value ? <Check size={12} /> : null}</DropdownMenuItem>)}<DropdownMenuItem disabled title="Latency bucket counts are not returned by the Overview metrics contract." className="h-[30px] rounded-lg px-[9px] text-xs">Latency distribution · unavailable</DropdownMenuItem></DropdownMenuContent></DropdownMenu>
}
