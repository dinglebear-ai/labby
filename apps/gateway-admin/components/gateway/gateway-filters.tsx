'use client'

import type { ReactNode } from 'react'
import { Check, Search, SlidersHorizontal, X } from 'lucide-react'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import {
  AURORA_CONTROL_SURFACE,
  AURORA_MEDIUM_PANEL,
  AURORA_MUTED_LABEL,
} from '@/components/aurora/tokens'
import { gatewayActionTone } from './gateway-theme'
import type {
  GatewaySourceFacet,
  GatewayStatusFacet,
  GatewayTransportFacet,
  ToolFilterState,
  ToolsExposureFilter,
} from './gateway-list-state'

export interface GatewayFiltersProps {
  mode: 'gateways' | 'tools'
  search: string
  gatewayFilters: {
    status: GatewayStatusFacet[]
    source: GatewaySourceFacet[]
    transport: GatewayTransportFacet[]
  }
  toolFilters: ToolFilterState
  gatewayOptions: Array<{ value: string; label: string }>
  mobileSheetOpen: boolean
  onMobileSheetOpenChange: (open: boolean) => void
  onSearchChange: (value: string) => void
  onGatewayFilterToggle: (group: 'status' | 'source' | 'transport', value: string) => void
  onToolFilterToggle: (group: 'gatewayIds' | 'source' | 'transport', value: string) => void
  onExposureChange: (value: ToolsExposureFilter) => void
  onClearFilters: () => void
}

interface FilterPillProps {
  active: boolean
  label: string
  onClick: () => void
  compact?: boolean
  /**
   * Overrides the button's accessible name. The Status group's labels
   * ("Healthy", "Enabled", "Disconnected") are the same words the Gateway
   * hero's stat chips use for their own toggle buttons elsewhere on the
   * page — screen-reader and voice-control users navigating by name can't
   * tell the two apart without this (bead lab-gxbhf). Leave unset for groups
   * whose labels are already unambiguous (Source, Transport, Exposure, ...).
   */
  ariaLabel?: string
}

const GATEWAY_STATUS_OPTIONS: Array<{ value: GatewayStatusFacet; label: string }> = [
  { value: 'configured', label: 'Configured' },
  { value: 'healthy', label: 'Healthy' },
  { value: 'disconnected', label: 'Disconnected' },
  { value: 'enabled', label: 'Enabled' },
  { value: 'disabled', label: 'Disabled' },
]

const SOURCE_OPTIONS: Array<{ value: GatewaySourceFacet; label: string }> = [
  { value: 'lab', label: 'Lab' },
  { value: 'custom', label: 'Custom' },
]

const TRANSPORT_OPTIONS: Array<{ value: GatewayTransportFacet; label: string }> = [
  { value: 'stdio', label: 'stdio' },
  { value: 'http', label: 'HTTP' },
]

const EXPOSURE_OPTIONS: Array<{ value: ToolsExposureFilter; label: string }> = [
  { value: 'all', label: 'All' },
  { value: 'exposed', label: 'Exposed only' },
  { value: 'hidden', label: 'Hidden only' },
]

function FilterGroup({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="space-y-2">
      <p className={AURORA_MUTED_LABEL}>{label}</p>
      <div className="flex flex-wrap gap-1.5">{children}</div>
    </div>
  )
}

function filterPillTone(active: boolean): string {
  return active
    ? 'border-aurora-accent-primary/45 bg-aurora-accent-primary/14 text-aurora-accent-strong shadow-[var(--aurora-active-glow)]'
    : 'border-aurora-border-strong bg-aurora-control-surface/75 text-aurora-text-muted shadow-[inset_0_1px_0_rgba(255,255,255,0.04)] hover:border-aurora-border-strong hover:bg-aurora-hover-bg hover:text-aurora-text-primary'
}

function FilterPill({ active, compact = false, label, onClick, ariaLabel }: FilterPillProps) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        'inline-flex shrink-0 items-center gap-1.5 whitespace-nowrap rounded-aurora-1 border font-semibold leading-none transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary/34',
        compact ? 'h-7 px-2 text-[11px]' : 'h-8 px-3 text-[12px]',
        filterPillTone(active),
      )}
      aria-pressed={active}
      aria-label={ariaLabel ?? label}
    >
      {active ? <Check className="size-3 text-aurora-accent-strong" /> : null}
      {label}
    </button>
  )
}

export function GatewayFilters({
  mode,
  search,
  gatewayFilters,
  toolFilters,
  gatewayOptions,
  mobileSheetOpen,
  onMobileSheetOpenChange,
  onSearchChange,
  onGatewayFilterToggle,
  onToolFilterToggle,
  onExposureChange,
  onClearFilters,
}: GatewayFiltersProps) {
  const gatewayHasNonSearchFilters =
    gatewayFilters.status.length > 0 ||
    gatewayFilters.source.length > 0 ||
    gatewayFilters.transport.length > 0

  const toolHasNonSearchFilters =
    toolFilters.gatewayIds.length > 0 ||
    toolFilters.exposure !== 'all' ||
    toolFilters.source.length > 0 ||
    toolFilters.transport.length > 0

  const hasFilters = mode === 'tools'
    ? search.length > 0 || toolHasNonSearchFilters
    : search.length > 0 || gatewayHasNonSearchFilters

  const activeMobilePills = mode === 'tools'
    ? [
        ...toolFilters.gatewayIds
          .map((gatewayId) => gatewayOptions.find((option) => option.value === gatewayId)?.label)
          .filter(Boolean) as string[],
        ...(toolFilters.exposure === 'all' ? [] : [EXPOSURE_OPTIONS.find((option) => option.value === toolFilters.exposure)?.label ?? toolFilters.exposure]),
        ...toolFilters.source.map((value) => SOURCE_OPTIONS.find((option) => option.value === value)?.label ?? value),
        ...toolFilters.transport.map((value) => TRANSPORT_OPTIONS.find((option) => option.value === value)?.label ?? value),
      ]
    : [
        ...gatewayFilters.status.map((value) => GATEWAY_STATUS_OPTIONS.find((option) => option.value === value)?.label ?? value),
        ...gatewayFilters.source.map((value) => SOURCE_OPTIONS.find((option) => option.value === value)?.label ?? value),
        ...gatewayFilters.transport.map((value) => TRANSPORT_OPTIONS.find((option) => option.value === value)?.label ?? value),
      ]

  const activeFilterCount = activeMobilePills.length + (search.length > 0 ? 1 : 0)
  const searchPlaceholder = mode === 'tools'
    ? 'Search tools, descriptions, or servers'
    : 'Search servers, commands, or endpoints'

  const filterGroups = (
    <div className="space-y-4">
      {mode === 'gateways' ? (
        <>
          <FilterGroup label="Status">
            {GATEWAY_STATUS_OPTIONS.map((option) => (
              <FilterPill
                key={option.value}
                active={gatewayFilters.status.includes(option.value)}
                label={option.label}
                ariaLabel={`${option.label} status filter`}
                onClick={() => onGatewayFilterToggle('status', option.value)}
              />
            ))}
          </FilterGroup>

          <FilterGroup label="Source">
            {SOURCE_OPTIONS.map((option) => (
              <FilterPill
                key={option.value}
                active={gatewayFilters.source.includes(option.value)}
                label={option.label}
                onClick={() => onGatewayFilterToggle('source', option.value)}
              />
            ))}
          </FilterGroup>

          <FilterGroup label="Transport">
            {TRANSPORT_OPTIONS.map((option) => (
              <FilterPill
                key={option.value}
                active={gatewayFilters.transport.includes(option.value)}
                label={option.label}
                onClick={() => onGatewayFilterToggle('transport', option.value)}
              />
            ))}
          </FilterGroup>
        </>
      ) : (
        <>
          <FilterGroup label="Server">
            {gatewayOptions.map((option) => (
              <FilterPill
                key={option.value}
                active={toolFilters.gatewayIds.includes(option.value)}
                label={option.label}
                onClick={() => onToolFilterToggle('gatewayIds', option.value)}
              />
            ))}
          </FilterGroup>

          <FilterGroup label="Exposure">
            {EXPOSURE_OPTIONS.map((option) => (
              <FilterPill
                key={option.value}
                active={toolFilters.exposure === option.value}
                label={option.label}
                onClick={() => onExposureChange(option.value)}
              />
            ))}
          </FilterGroup>

          <FilterGroup label="Source">
            {SOURCE_OPTIONS.map((option) => (
              <FilterPill
                key={option.value}
                active={toolFilters.source.includes(option.value)}
                label={option.label}
                onClick={() => onToolFilterToggle('source', option.value)}
              />
            ))}
          </FilterGroup>

          <FilterGroup label="Transport">
            {TRANSPORT_OPTIONS.map((option) => (
              <FilterPill
                key={option.value}
                active={toolFilters.transport.includes(option.value)}
                label={option.label}
                onClick={() => onToolFilterToggle('transport', option.value)}
              />
            ))}
          </FilterGroup>
        </>
      )}
    </div>
  )

  return (
    <>
      <div className="space-y-3 lg:hidden">
        <div
          data-mobile-search={mode}
          className={cn(
            AURORA_CONTROL_SURFACE,
            'relative flex h-11 items-center gap-2 border px-2 transition-shadow focus-within:border-aurora-accent-primary/45 focus-within:shadow-[var(--aurora-active-glow)]',
          )}
        >
          <div className="grid size-7 shrink-0 place-items-center rounded-aurora-1 border border-aurora-border-strong/70 bg-aurora-panel-strong/60 text-aurora-accent-strong">
            <Search className="size-3.5" />
          </div>
          <Input
            aria-label={mode === 'tools' ? 'Search tools' : 'Search servers'}
            name={mode === 'tools' ? 'gateway-tools-search-mobile' : 'gateways-search-mobile'}
            placeholder={mode === 'tools' ? 'Search tools' : 'Search servers'}
            value={search}
            onChange={(e) => onSearchChange(e.target.value)}
            className="h-9 min-w-0 flex-1 border-0 bg-transparent px-0 text-[14px] text-aurora-text-primary shadow-none placeholder:text-aurora-text-muted focus-visible:ring-0"
          />
          {search ? (
            <Button
              type="button"
              variant="outline"
              size="icon"
              onClick={() => onSearchChange('')}
              className={cn(gatewayActionTone(), 'size-7 rounded-aurora-1 hover:bg-aurora-hover-bg hover:text-aurora-text-primary')}
              aria-label="Clear search"
            >
              <X className="size-3.5" />
            </Button>
          ) : null}
          <div className="relative">
            <Button
              type="button"
              variant="outline"
              size="icon"
              onClick={() => onMobileSheetOpenChange(!mobileSheetOpen)}
              className={cn(gatewayActionTone(), 'relative size-7 rounded-aurora-1 text-aurora-accent-strong hover:bg-aurora-hover-bg hover:text-aurora-text-primary')}
              aria-label="Open filters"
            >
              <SlidersHorizontal className="size-3.5" />
              {activeFilterCount > 0 ? (
                <span className="absolute -top-1 -right-1 rounded-full border border-aurora-accent-primary/35 bg-aurora-accent-primary/14 px-1.5 text-[10px] font-semibold leading-4 text-aurora-accent-strong">
                  {activeFilterCount}
                </span>
              ) : null}
            </Button>
          </div>
        </div>

        {activeMobilePills.length > 0 ? (
          <div className="flex gap-1.5 overflow-x-auto pb-0.5">
            {activeMobilePills.map((label) => (
              <span
                key={label}
                className={cn(
                  'inline-flex h-7 shrink-0 items-center rounded-aurora-1 border px-2.5 text-[10px] font-bold uppercase tracking-[0.12em]',
                  filterPillTone(true),
                )}
              >
                {label}
              </span>
            ))}
          </div>
        ) : null}

        {mobileSheetOpen ? (
          <div className={cn(AURORA_MEDIUM_PANEL, 'space-y-4 rounded-aurora-1 p-4')}>
            <div className="flex items-center justify-between gap-3">
              <div>
                <p className={AURORA_MUTED_LABEL}>Filters</p>
                <p className="mt-1 text-xs font-medium text-aurora-text-muted">
                  {activeFilterCount > 0 ? `${activeFilterCount} active` : 'No filters active'}
                </p>
              </div>
              {hasFilters ? (
                <Button
                  variant="outline"
                  size="sm"
                  onClick={onClearFilters}
                  className={cn(gatewayActionTone(), 'h-8 px-3 text-aurora-accent-strong hover:bg-aurora-hover-bg hover:text-aurora-text-primary')}
                >
                  <X className="mr-1 size-4" />
                  Clear filters
                </Button>
              ) : null}
            </div>
            {filterGroups}
          </div>
        ) : null}
      </div>

      <div className={cn(AURORA_MEDIUM_PANEL, 'hidden rounded-aurora-1 bg-[linear-gradient(180deg,rgba(18,40,56,0.86),rgba(12,27,38,0.96))] p-2.5 lg:block')}>
        <div
          className={cn(
            AURORA_CONTROL_SURFACE,
            'relative flex h-11 w-full items-center gap-2 border px-2 transition-shadow focus-within:border-aurora-accent-primary/45 focus-within:shadow-[var(--aurora-active-glow)]',
          )}
        >
          <div className="grid size-7 shrink-0 place-items-center rounded-aurora-1 border border-aurora-border-strong/70 bg-aurora-panel-strong/60 text-aurora-accent-strong">
            <Search className="size-3.5" />
          </div>
          <Input
            aria-label={mode === 'tools' ? 'Search tools' : 'Search servers'}
            name={mode === 'tools' ? 'gateway-tools-search' : 'gateways-search'}
            placeholder={searchPlaceholder}
            value={search}
            onChange={(e) => onSearchChange(e.target.value)}
            className="h-9 min-w-0 flex-1 border-0 bg-transparent px-0 text-[13px] text-aurora-text-primary shadow-none placeholder:text-aurora-text-muted focus-visible:ring-0"
          />
          {search ? (
            <Button
              type="button"
              variant="outline"
              size="icon"
              onClick={() => onSearchChange('')}
              className={cn(gatewayActionTone(), 'size-7 rounded-aurora-1 hover:bg-aurora-hover-bg hover:text-aurora-text-primary')}
              aria-label="Clear search"
            >
              <X className="size-3.5" />
            </Button>
          ) : null}
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => onMobileSheetOpenChange(!mobileSheetOpen)}
            className={cn(gatewayActionTone(), 'h-8 shrink-0 gap-2 rounded-aurora-1 text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary')}
            aria-expanded={mobileSheetOpen}
            aria-label="Toggle filters"
          >
            <SlidersHorizontal className="size-3.5" />
            Filters
            {activeFilterCount > 0 ? (
              <span className="grid h-5 min-w-5 place-items-center rounded-full bg-aurora-accent-primary/15 px-1.5 text-[10px] font-bold text-aurora-accent-strong">
                {activeFilterCount}
              </span>
            ) : null}
          </Button>
        </div>
        {mobileSheetOpen ? (
          <div className="mt-3 border-t border-aurora-border-subtle px-2 pt-3">
            <div className="mb-3 flex items-center justify-between">
              <p className={AURORA_MUTED_LABEL}>{activeFilterCount > 0 ? `${activeFilterCount} active filters` : 'Filter results'}</p>
              {hasFilters ? (
            <Button
              variant="outline"
              size="sm"
              onClick={onClearFilters}
              className={cn(gatewayActionTone(), 'h-8 gap-1.5 rounded-aurora-1 text-aurora-accent-strong hover:bg-aurora-hover-bg hover:text-aurora-text-primary')}
              aria-label="Clear filters"
            >
              <X className="size-3.5" />
              Clear
            </Button>
              ) : null}
            </div>
            {filterGroups}
          </div>
        ) : null}
      </div>
    </>
  )
}
