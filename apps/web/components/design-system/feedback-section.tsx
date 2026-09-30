'use client'

import { useState } from 'react'
import {
  AlertCircle,
  CheckCircle2,
  FolderSearch,
  ShieldAlert,
} from 'lucide-react'

import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from '@/components/ui/empty'
import { Skeleton } from '@/components/ui/skeleton'
import { Spinner } from '@/components/ui/spinner'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import {
  AURORA_ACCENT_CTA,
  AURORA_DISPLAY_2,
  AURORA_MEDIUM_PANEL,
  AURORA_MUTED_LABEL,
  AURORA_STRONG_PANEL,
  controlTone,
} from '@/components/aurora/tokens'
import { feedbackModes } from './demo-data'

export function FeedbackSection() {
  const [activeMode, setActiveMode] = useState<(typeof feedbackModes)[number]>('success')

  return (
    <section className={cn(AURORA_STRONG_PANEL, 'overflow-hidden')}>
      <div className="border-b border-aurora-border-strong px-5 py-4">
        <p className={AURORA_MUTED_LABEL}>Feedback</p>
        <h2 className={cn(AURORA_DISPLAY_2, 'mt-2 text-aurora-text-primary')}>
          Response states
        </h2>
        <p className="mt-2 max-w-2xl text-sm text-aurora-text-muted">
          Keep async, empty, and status messaging visually consistent across setup, logs, and
          gateway workflows.
        </p>
      </div>

      <div className="space-y-4 px-5 py-5">
        <div className="flex flex-wrap gap-2">
          {feedbackModes.map((mode) => (
            <Button
              key={mode}
              variant="outline"
              aria-pressed={mode === activeMode}
              className={cn(
                controlTone(mode === activeMode ? 'accent' : 'default'),
                'rounded-full px-3 py-1.5 text-sm capitalize hover:bg-aurora-hover-bg hover:text-aurora-text-primary',
              )}
              onClick={() => setActiveMode(mode)}
            >
              {mode}
            </Button>
          ))}
        </div>

        <div className="grid gap-4 xl:grid-cols-[minmax(0,1.05fr)_minmax(0,0.95fr)]">
          <div className="space-y-4">
            <div className={cn(AURORA_MEDIUM_PANEL, 'space-y-4 px-4 py-4')}>
              <div className="flex items-center justify-between gap-3">
                <p className="text-sm font-medium text-aurora-text-primary">Loading</p>
                <Badge variant="secondary">Live preview</Badge>
              </div>
              <div className="flex items-center gap-3 rounded-[1rem] border border-aurora-border-strong bg-aurora-control-surface px-4 py-4">
                <Spinner className="size-5 text-aurora-accent-primary" />
                <div>
                  <p className="text-sm font-medium text-aurora-text-primary">Rebuilding preview</p>
                  <p className="text-sm text-aurora-text-muted">Local-only async indicators should stay calm and obvious.</p>
                </div>
              </div>
              <div className="grid gap-2 sm:grid-cols-3">
                <Skeleton className="h-18 rounded-aurora-2 bg-aurora-panel-medium" />
                <Skeleton className="h-18 rounded-aurora-2 bg-aurora-panel-medium" />
                <Skeleton className="h-18 rounded-aurora-2 bg-aurora-panel-medium" />
              </div>
            </div>

            <div className={cn(AURORA_MEDIUM_PANEL, 'px-4 py-4')}>
              <p className="mb-3 text-sm font-medium text-aurora-text-primary">Empty state</p>
              <Empty className="rounded-[1rem] border-aurora-border-strong bg-aurora-control-surface/60 text-aurora-text-primary">
                <EmptyHeader>
                  <EmptyMedia variant="icon" className="bg-aurora-panel-medium text-aurora-accent-strong">
                    <FolderSearch className="size-5" />
                  </EmptyMedia>
                  <EmptyTitle>Empty state</EmptyTitle>
                  <EmptyDescription>
                    No matching gateways in this local sandbox snapshot. Adjust filters or seed more
                    demo data.
                  </EmptyDescription>
                </EmptyHeader>
                <EmptyContent>
                  <Button className={AURORA_ACCENT_CTA}>
                    Add sample data
                  </Button>
                </EmptyContent>
              </Empty>
            </div>
          </div>

          <div className="space-y-4">
            <Alert
              variant="default"
              className={cn('transition-opacity', activeMode === 'success' ? 'opacity-100' : 'opacity-70')}
            >
              <CheckCircle2 className="size-4" />
              <AlertTitle>Success</AlertTitle>
              <AlertDescription>
                Preview state updated. No network request left this page.
              </AlertDescription>
            </Alert>
            <Alert
              variant="warn"
              className={cn('transition-opacity', activeMode === 'warning' ? 'opacity-100' : 'opacity-70')}
            >
              <ShieldAlert className="size-4" />
              <AlertTitle>Warning</AlertTitle>
              <AlertDescription>
                A warning should feel actionable without looking like a hard failure.
              </AlertDescription>
            </Alert>
            <Alert
              variant="error"
              className={cn('transition-opacity', activeMode === 'error' ? 'opacity-100' : 'opacity-70')}
            >
              <AlertCircle className="size-4" />
              <AlertTitle>Error</AlertTitle>
              <AlertDescription>
                Local demo failure: confirm the destructive state copy and recovery affordances.
              </AlertDescription>
            </Alert>
          </div>
        </div>

        <div className={cn(AURORA_MEDIUM_PANEL, 'space-y-4 px-4 py-4')}>
          <div className="flex items-center justify-between gap-3">
            <p className="text-sm font-medium text-aurora-text-primary">Badge status matrix</p>
            <p className={AURORA_MUTED_LABEL}>variant x status</p>
          </div>
          <div className="grid gap-3 md:grid-cols-[100px_repeat(4,minmax(0,1fr))]">
            <div aria-hidden />
            <p className={AURORA_MUTED_LABEL}>default</p>
            <p className={AURORA_MUTED_LABEL}>warn</p>
            <p className={AURORA_MUTED_LABEL}>error</p>
            <p className={AURORA_MUTED_LABEL}>success</p>

            <p className="text-sm font-medium text-aurora-text-primary">default</p>
            <Badge variant="default">Default</Badge>
            <Badge variant="default" status="warn">Warn</Badge>
            <Badge variant="default" status="error">Error</Badge>
            <Badge variant="default" status="success">Success</Badge>

            <p className="text-sm font-medium text-aurora-text-primary">outline</p>
            <Badge variant="outline">Default</Badge>
            <Badge variant="outline" status="warn">Warn</Badge>
            <Badge variant="outline" status="error">Error</Badge>
            <Badge variant="outline" status="success">Success</Badge>

            <p className="text-sm font-medium text-aurora-text-primary">pill</p>
            <Badge variant="pill">Default</Badge>
            <Badge variant="pill" status="warn">Warn</Badge>
            <Badge variant="pill" status="error">Error</Badge>
            <Badge variant="pill" status="success">Success</Badge>
          </div>
        </div>
      </div>
    </section>
  )
}
