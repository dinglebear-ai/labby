'use client'

import {
  AlertTriangle,
  Cable,
  Search,
  ShieldCheck,
  SlidersHorizontal,
  UserRoundCheck,
  Wrench,
} from 'lucide-react'

import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { cn } from '@/lib/utils'
import {
  AURORA_DISPLAY_2,
  AURORA_MEDIUM_PANEL,
  AURORA_MESSAGE_SURFACE,
  AURORA_MUTED_LABEL,
  AURORA_STRONG_PANEL,
  controlTone,
} from '@/components/aurora/tokens'
import { toolbarSummaryChips } from './demo-data'

export function PatternsSection() {
  return (
    <section className={cn(AURORA_STRONG_PANEL, 'overflow-hidden')}>
      <div className="border-b border-aurora-border-strong px-5 py-4">
        <p className={AURORA_MUTED_LABEL}>Application Patterns</p>
        <h2 className={cn(AURORA_DISPLAY_2, 'mt-2 text-aurora-text-primary')}>
          Composed admin workflows
        </h2>
        <p className="mt-2 max-w-2xl text-sm text-aurora-text-muted">
          Stress the combinations the product uses most often: gateway toolbars and auth/setup
          messaging.
        </p>
      </div>

      <div className="space-y-4 px-5 py-5">
        <div className="grid gap-4 xl:grid-cols-[minmax(0,1.05fr)_minmax(0,0.95fr)]">
          <div className={cn(AURORA_MEDIUM_PANEL, 'space-y-4 px-4 py-4')}>
            <p className="text-sm font-medium text-aurora-text-primary">Gateway toolbar pattern</p>
            <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
              {toolbarSummaryChips.map((chip) => (
                <button key={chip.label} type="button" aria-label={chip.label} className="inline-flex items-center justify-center gap-1 rounded-aurora-1 border border-aurora-border-strong bg-aurora-control-surface px-2 py-2 text-sm font-medium text-aurora-text-primary">
                  {chip.tone === 'success' ? <ShieldCheck className="size-4 text-aurora-success" /> : null}
                  {chip.tone === 'warning' ? <AlertTriangle className="size-4 text-aurora-warn" /> : null}
                  {chip.tone === 'info' ? <Wrench className="size-4 text-aurora-accent-primary" /> : null}
                  {chip.tone === 'default' ? <Cable className="size-4 text-aurora-text-muted" /> : null}
                  <span className="font-semibold">{chip.label.split(' ')[0]}</span>
                </button>
              ))}
            </div>
            <div className="space-y-2">
              <label className="relative block">
                <Search className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-aurora-text-muted" />
                <Input
                  aria-label="Search gateways"
                  defaultValue="plex"
                  className={cn(AURORA_MESSAGE_SURFACE, 'pr-20 pl-9 text-aurora-text-primary placeholder:text-aurora-text-muted')}
                />
                <div className="absolute inset-y-0 right-1 flex items-center gap-1">
                  <Button variant="outline" size="icon" aria-label="Open gateway filters" className={cn(controlTone(), 'size-8 rounded-full hover:bg-aurora-hover-bg hover:text-aurora-text-primary')}>
                    <SlidersHorizontal className="size-3.5" />
                  </Button>
                  <Button variant="outline" size="icon" aria-label="Open gateway tools" className={cn(controlTone(), 'size-8 rounded-full hover:bg-aurora-hover-bg hover:text-aurora-text-primary')}>
                    <Wrench className="size-3.5" />
                  </Button>
                </div>
              </label>
              <div className="flex flex-wrap gap-2">
                <Button variant="outline" size="sm" className={cn(controlTone(), 'rounded-full hover:bg-aurora-hover-bg hover:text-aurora-text-primary')}>
                  Healthy
                </Button>
                <Button variant="outline" size="sm" className={cn(controlTone(), 'rounded-full hover:bg-aurora-hover-bg hover:text-aurora-text-primary')}>
                  HTTP
                </Button>
                <Button className="ml-auto rounded-aurora-1 bg-aurora-accent-primary text-primary-foreground hover:bg-aurora-accent-strong">
                  Add gateway
                </Button>
              </div>
            </div>
          </div>

          <div className={cn(AURORA_MEDIUM_PANEL, 'space-y-4 px-4 py-4')}>
            <p className="text-sm font-medium text-aurora-text-primary">Auth and setup states</p>
            <div className="grid gap-3 md:grid-cols-2">
              <div className="rounded-[1rem] border border-aurora-border-strong bg-aurora-control-surface px-4 py-4">
                <div className="flex items-center gap-2">
                  <UserRoundCheck className="size-4 text-aurora-success" />
                  <p className="font-medium text-aurora-text-primary">Signed in session</p>
                </div>
                <p className="mt-2 text-sm text-aurora-text-muted">
                  labby-admin@example.com is operating in preview mode with session auth.
                </p>
                <Badge variant="secondary" className="mt-3">Healthy auth</Badge>
              </div>
              <div className="rounded-[1rem] border border-aurora-error/30 bg-aurora-error/10 px-4 py-4">
                <div className="flex items-center gap-2">
                  <AlertTriangle className="size-4 text-aurora-error" />
                  <p className="font-medium text-aurora-error">Setup attention</p>
                </div>
                <p className="mt-2 text-sm text-aurora-text-muted">
                  One sample service is missing a token. Keep recovery copy direct and close to the action.
                </p>
                <Badge variant="outline" className="mt-3 border-aurora-error/40 text-aurora-error">Needs action</Badge>
              </div>
            </div>
          </div>
        </div>
      </div>
    </section>
  )
}
