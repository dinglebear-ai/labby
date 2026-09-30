'use client'

import { useState } from 'react'
import { Check, Trash2, WandSparkles } from 'lucide-react'

import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { cn } from '@/lib/utils'
import {
  AURORA_ACCENT_CTA,
  AURORA_CONTROL_SURFACE,
  AURORA_DISPLAY_2,
  AURORA_MEDIUM_PANEL,
  AURORA_MUTED_LABEL,
  AURORA_STRONG_PANEL,
  controlTone,
  pillTone,
} from '@/components/aurora/tokens'
import {
  accessModeOptions,
  environmentOptions,
  pillFilterOptions,
} from './demo-data'
import { useDemoAsync } from './demo-state'

export function ControlsSection() {
  const [environment, setEnvironment] = useState<string>(environmentOptions[0].value)
  const [query, setQuery] = useState('gateway.status:healthy')
  const [notes, setNotes] = useState('Roll out the refreshed admin affordances behind a hidden route.')
  const [accessMode, setAccessMode] = useState<string>(accessModeOptions[0].value)
  const [autoRefresh, setAutoRefresh] = useState(true)
  const [notifyOwners, setNotifyOwners] = useState(true)
  const [pillFilters, setPillFilters] = useState<string[]>([
    pillFilterOptions[0],
    pillFilterOptions[2],
  ])
  const saveDemo = useDemoAsync({
    idle: 'Ready to simulate a local save.',
    loading: 'Simulating local save…',
    success: 'Local save preview completed.',
    error: 'Save preview failed locally.',
  })
  const destructiveDemo = useDemoAsync({
    idle: 'No destructive action has been triggered.',
    loading: 'Simulating destructive confirmation…',
    success: 'Preview cleanup completed locally.',
    error: 'Local destructive demo returned an error state.',
  })

  return (
    <section className={cn(AURORA_STRONG_PANEL, 'overflow-hidden')}>
      <div className="border-b border-aurora-border-strong px-5 py-4">
        <p className={AURORA_MUTED_LABEL}>Controls</p>
        <h2 className={cn(AURORA_DISPLAY_2, 'mt-2 text-aurora-text-primary')}>
          Form and action primitives
        </h2>
        <p className="mt-2 max-w-2xl text-sm text-aurora-text-muted">
          Exercise real app controls in one place, with local state only and fake async outcomes.
        </p>
      </div>

      <div className="grid gap-4 px-5 py-5 xl:grid-cols-[minmax(0,1.15fr)_minmax(0,0.85fr)]">
        <div className="space-y-4">
          <div className={cn(AURORA_MEDIUM_PANEL, 'space-y-4 px-4 py-4')}>
            <p className={AURORA_MUTED_LABEL}>Button hierarchy</p>
            <div className="flex flex-wrap items-center gap-3">
              <Button>Default</Button>
              <Button variant="outline">Outline</Button>
              <Button variant="secondary">Secondary</Button>
              <Button variant="ghost">Ghost</Button>
              <Button variant="destructive">Destructive</Button>
              <Button variant="link">Link</Button>
            </div>
            <p className={AURORA_MUTED_LABEL}>data-selected glow</p>
            <div className="flex flex-wrap items-center gap-3">
              <Button data-selected="true">Selected (glow)</Button>
              <Button data-selected="false">Unselected</Button>
              <Button variant="outline" data-selected="true">Outline selected</Button>
            </div>
            <p className={AURORA_MUTED_LABEL}>Input, Select, Textarea states</p>
            <div className="grid gap-4 md:grid-cols-3">
              <div className="space-y-2">
                <Label htmlFor="design-input-default" className="text-aurora-text-primary">
                  Default input
                </Label>
                <Input id="design-input-default" defaultValue="healthy" />
              </div>
              <div className="space-y-2">
                <Label htmlFor="design-input-focused" className="text-aurora-text-primary">
                  Focused-looking input
                </Label>
                <Input id="design-input-focused" defaultValue="focus preview" autoFocus={false} className="ring-aurora-focus-ring/40 ring-2" />
              </div>
              <div className="space-y-2">
                <Label htmlFor="design-input-error" className="text-aurora-text-primary">
                  Invalid input
                </Label>
                <Input id="design-input-error" defaultValue="bad-value" aria-invalid="true" />
              </div>
            </div>
            <div className="grid gap-4 md:grid-cols-2">
              <div className="space-y-2">
                <Label htmlFor="design-query" className="text-aurora-text-primary">
                  Search input
                </Label>
                <Input
                  id="design-query"
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  className={cn(AURORA_CONTROL_SURFACE, 'border-aurora-border-strong text-aurora-text-primary')}
                />
              </div>
              <div className="space-y-2">
                <Label className="text-aurora-text-primary">Environment select</Label>
                <Select value={environment} onValueChange={setEnvironment}>
                  <SelectTrigger className={cn(AURORA_CONTROL_SURFACE, 'w-full border-aurora-border-strong text-aurora-text-primary')}>
                    <SelectValue placeholder="Choose environment" />
                  </SelectTrigger>
                  <SelectContent>
                    {environmentOptions.map((option) => (
                      <SelectItem key={option.value} value={option.value}>
                        {option.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            </div>
            <div className="space-y-2">
              <Label htmlFor="design-notes" className="text-aurora-text-primary">
                Textarea
              </Label>
              <Textarea
                id="design-notes"
                value={notes}
                onChange={(event) => setNotes(event.target.value)}
                className={cn(AURORA_CONTROL_SURFACE, 'min-h-28 border-aurora-border-strong text-aurora-text-primary')}
              />
            </div>
          </div>

          <div className="grid gap-4 lg:grid-cols-2">
            <div className={cn(AURORA_MEDIUM_PANEL, 'space-y-3 px-4 py-4')}>
              <p className="text-sm font-medium text-aurora-text-primary">Pill filters</p>
              <div className="flex flex-wrap gap-2">
                {pillFilterOptions.map((option) => {
                  const active = pillFilters.includes(option)
                  return (
                    <button
                      key={option}
                      type="button"
                      aria-pressed={active}
                      aria-label={`Toggle ${option}`}
                      className={`inline-flex min-h-[38px] items-center rounded-full border px-3.5 py-1.5 text-sm font-medium transition ${pillTone(active)}`}
                      onClick={() => {
                        setPillFilters((current) =>
                          current.includes(option)
                            ? current.filter((item) => item !== option)
                            : [...current, option],
                        )
                      }}
                    >
                      {option}
                    </button>
                  )
                })}
              </div>
            </div>

            <div className={cn(AURORA_MEDIUM_PANEL, 'space-y-3 px-4 py-4')}>
              <p className="text-sm font-medium text-aurora-text-primary">Selection controls</p>
              <div className="space-y-3">
                <Label className="justify-between text-aurora-text-primary">
                  <span className="flex items-center gap-2">
                    <Checkbox checked={notifyOwners} onCheckedChange={(checked) => setNotifyOwners(checked === true)} aria-label="Notify owners" />
                    Notify owners
                  </span>
                  <Badge variant="secondary">{notifyOwners ? 'On' : 'Off'}</Badge>
                </Label>
                <Label className="justify-between text-aurora-text-primary">
                  <span>Auto-refresh panels</span>
                  <Switch checked={autoRefresh} onCheckedChange={setAutoRefresh} aria-label="Auto-refresh panels" />
                </Label>
                <RadioGroup value={accessMode} onValueChange={setAccessMode}>
                  {accessModeOptions.map((option) => (
                    <Label key={option.value} className="justify-between rounded-aurora-2 border border-aurora-border-strong bg-aurora-control-surface/70 px-3 py-3 text-aurora-text-primary">
                      <span>{option.label}</span>
                      <RadioGroupItem value={option.value} aria-label={option.label} />
                    </Label>
                  ))}
                </RadioGroup>
              </div>
            </div>
          </div>
        </div>

        <div className="space-y-4">
          <div className={cn(AURORA_MEDIUM_PANEL, 'space-y-3 px-4 py-4')}>
            <p className="text-sm font-medium text-aurora-text-primary">Loading state</p>
            <p className="text-sm text-aurora-text-muted">{saveDemo.message}</p>
            <div className="flex flex-wrap gap-3">
              <Button
                className={AURORA_ACCENT_CTA}
                disabled={saveDemo.status === 'loading'}
                onClick={() => saveDemo.run('success')}
              >
                {saveDemo.status === 'loading' ? (
                  <>
                    <WandSparkles className="size-4 animate-pulse" />
                    Saving…
                  </>
                ) : (
                  <>
                    <Check className="size-4" />
                    Simulate success
                  </>
                )}
              </Button>
              <Button
                variant="outline"
                className={cn(controlTone(), 'rounded-aurora-1 hover:bg-aurora-hover-bg hover:text-aurora-text-primary')}
                disabled={saveDemo.status === 'loading'}
                onClick={saveDemo.reset}
              >
                Reset
              </Button>
            </div>
          </div>

          <div className={cn(AURORA_MEDIUM_PANEL, 'space-y-3 px-4 py-4')}>
            <p className="text-sm font-medium text-aurora-text-primary">Destructive action</p>
            <p className="text-sm text-aurora-text-muted">{destructiveDemo.message}</p>
            <div className="flex flex-wrap gap-3">
              <Button
                variant="destructive"
                className="rounded-aurora-1"
                disabled={destructiveDemo.status === 'loading'}
                onClick={() => destructiveDemo.run('error')}
              >
                <Trash2 className="size-4" />
                Clear preview cache
              </Button>
              <Button
                variant="outline"
                className={cn(controlTone(), 'rounded-aurora-1 hover:bg-aurora-hover-bg hover:text-aurora-text-primary')}
                disabled={destructiveDemo.status === 'loading'}
                onClick={() => destructiveDemo.run('success')}
              >
                Confirm safe variant
              </Button>
            </div>
          </div>
        </div>
      </div>
    </section>
  )
}
