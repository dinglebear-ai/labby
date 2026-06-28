'use client'

import { useState } from 'react'
import { Search, WandSparkles } from 'lucide-react'

import { AppHeader } from '@/components/app-header'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Spinner } from '@/components/ui/spinner'
import { setupResultDescription, SetupResults } from './setup-results'
import { SetupStatGrid } from './setup-stat-grid'
import { useExtractScan } from './use-extract-scan'
import { cn } from '@/lib/utils'
import {
  AURORA_DISPLAY_1,
  AURORA_MEDIUM_PANEL,
  AURORA_MUTED_LABEL,
  AURORA_PAGE_FRAME,
  AURORA_PAGE_SHELL,
  AURORA_STRONG_PANEL,
} from '@/components/aurora/tokens'

export function SetupPageContent() {
  const [targetUri, setTargetUri] = useState('')
  const { error, isLoading, report, runScan, sortedCreds, summary } = useExtractScan()

  return (
    <>
      <AppHeader breadcrumbs={[{ label: 'Setup' }]} />

      <div className={cn(AURORA_PAGE_SHELL, 'flex-1')}>
        <div className={cn(AURORA_PAGE_FRAME, 'space-y-6')}>
          <section className={cn(AURORA_STRONG_PANEL, 'space-y-3 p-5')}>
            <p className={AURORA_MUTED_LABEL}>Environment bootstrap</p>
            <h1 className={cn(AURORA_DISPLAY_1, 'text-aurora-text-primary')}>Service Setup</h1>
            <p className="max-w-3xl text-sm leading-[1.55] text-aurora-text-muted">
              Run discovery against the fleet or a single target path without leaving the Aurora admin shell.
            </p>
          </section>

          <div className="grid gap-4 xl:grid-cols-[minmax(0,1.2fr)_minmax(0,0.8fr)]">
            <Card className={cn('gap-0', AURORA_MEDIUM_PANEL)}>
              <CardHeader className="border-b">
                <CardTitle>Environment discovery</CardTitle>
                <CardDescription>
                  Run the existing <code>extract.scan</code> flow from the web UI. Fleet scan checks every
                  actionable SSH host; targeted scan lets you point at a specific appdata root.
                </CardDescription>
              </CardHeader>
              <CardContent className="space-y-6 pt-6">
                <div className="flex flex-wrap gap-3">
                  <Button disabled={isLoading} onClick={() => void runScan()}>
                    {isLoading ? <Spinner className="mr-2" /> : <WandSparkles className="mr-2 size-4" />}
                    Fleet Scan
                  </Button>
                </div>

                <div className="space-y-3 rounded-lg border bg-aurora-control-surface/20 p-4">
                  <div>
                    <p className="text-sm font-medium">Targeted scan</p>
                    <p className="text-sm text-aurora-text-muted">
                      Use a local path or SSH target like <code>node-b:/mnt/appdata</code>.
                    </p>
                  </div>
                  <div className="flex flex-col gap-3 sm:flex-row">
                    <Input
                      aria-label="Scan target path"
                      name="setup-target-path"
                      placeholder="host:/absolute/path or /local/path"
                      value={targetUri}
                      onChange={(event) => setTargetUri(event.target.value)}
                    />
                    <Button
                      disabled={isLoading || targetUri.trim().length === 0}
                      onClick={() => void runScan({ uri: targetUri.trim() })}
                      variant="secondary"
                    >
                      {isLoading ? <Spinner className="mr-2" /> : <Search className="mr-2 size-4" />}
                      Targeted Scan
                    </Button>
                  </div>
                </div>

                {error ? (
                  <div className="rounded-lg border border-aurora-error/30 bg-aurora-error/10 p-4 text-sm text-aurora-error">
                    {error}
                  </div>
                ) : null}
              </CardContent>
            </Card>

            <SetupStatGrid summary={summary} />
          </div>

          <Card className={AURORA_MEDIUM_PANEL}>
            <CardHeader className="border-b">
              <CardTitle>Last scan</CardTitle>
              <CardDescription>{setupResultDescription(report)}</CardDescription>
            </CardHeader>
            <CardContent className="pt-6">
              <SetupResults report={report} sortedCreds={sortedCreds} />
            </CardContent>
          </Card>
        </div>
      </div>
    </>
  )
}
