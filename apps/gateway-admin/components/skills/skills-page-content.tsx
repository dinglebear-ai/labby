'use client'

import { useCallback, useEffect, useState } from 'react'
import { AlertTriangle, BookOpen, Loader2, RefreshCw, Scissors, ShieldAlert } from 'lucide-react'

import { AppHeader } from '@/components/app-header'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  AURORA_CARD_TITLE,
  AURORA_DENSE_META,
  AURORA_DISPLAY_1,
  AURORA_DISPLAY_NUMBER,
  AURORA_MEDIUM_PANEL,
  AURORA_MUTED_LABEL,
  AURORA_PAGE_FRAME,
  AURORA_PAGE_SHELL,
  AURORA_STRONG_PANEL,
} from '@/components/aurora/tokens'
import { gatewayAction } from '@/lib/api/gateway-client'
import {
  formatCacheAge,
  skillsRowStatus,
  skillsRowSummary,
  sortSkillsRows,
  totalSkillCount,
  type SkillsRowStatus,
  type UpstreamSkillsRow,
} from '@/lib/api/skills-model'
import { cn } from '@/lib/utils'

type LoadState =
  | { kind: 'loading' }
  | { kind: 'loaded'; rows: UpstreamSkillsRow[] }
  | { kind: 'error'; detail: string }

/** Icon and tone per row status, so severity reads before the text does. */
function statusPresentation(status: SkillsRowStatus) {
  switch (status) {
    case 'error':
      return { Icon: ShieldAlert, tone: 'text-destructive', label: 'Unreachable' }
    case 'truncated':
      return { Icon: Scissors, tone: 'text-amber-500', label: 'Truncated' }
    case 'excluded':
      return { Icon: AlertTriangle, tone: 'text-amber-500', label: 'Partly excluded' }
    case 'empty':
      return { Icon: BookOpen, tone: 'text-muted-foreground', label: 'No listing' }
    case 'ok':
      return { Icon: BookOpen, tone: 'text-muted-foreground', label: 'Healthy' }
  }
}

export function SkillsPageContent() {
  const [state, setState] = useState<LoadState>({ kind: 'loading' })

  const load = useCallback(async (signal?: AbortSignal) => {
    setState({ kind: 'loading' })
    try {
      const rows = await gatewayAction<UpstreamSkillsRow[]>('gateway.skills.list', {}, signal)
      setState({ kind: 'loaded', rows: sortSkillsRows(rows ?? []) })
    } catch (error) {
      // An aborted request is a navigation, not a failure to report.
      if (signal?.aborted) return
      setState({ kind: 'error', detail: error instanceof Error ? error.message : String(error) })
    }
  }, [])

  useEffect(() => {
    const controller = new AbortController()
    void load(controller.signal)
    return () => controller.abort()
  }, [load])

  return (
    <div className={AURORA_PAGE_SHELL}>
      <AppHeader title="Skills" />
      <div className={AURORA_PAGE_FRAME}>
        <div className="flex items-start justify-between gap-4">
          <div>
            <h1 className={AURORA_DISPLAY_1}>Agent Skills</h1>
            <p className={cn(AURORA_MUTED_LABEL, 'mt-1 max-w-2xl')}>
              Skills aggregated from upstreams with <code>proxy_skills</code> enabled. Labby serves
              its own skills separately under the reserved <code>labby</code> origin.
            </p>
          </div>
          <Button
            variant="outline"
            size="sm"
            onClick={() => void load()}
            disabled={state.kind === 'loading'}
          >
            {state.kind === 'loading' ? (
              <Loader2 className="size-4 animate-spin" />
            ) : (
              <RefreshCw className="size-4" />
            )}
            Refresh
          </Button>
        </div>

        {state.kind === 'loading' && (
          <div className={cn(AURORA_MEDIUM_PANEL, 'mt-6 flex items-center gap-3 p-6')}>
            <Loader2 className="size-4 animate-spin" />
            <span className={AURORA_MUTED_LABEL}>Loading skills…</span>
          </div>
        )}

        {state.kind === 'error' && (
          <div className={cn(AURORA_STRONG_PANEL, 'mt-6 p-6')}>
            <div className="flex items-center gap-2">
              <ShieldAlert className="size-4 text-destructive" />
              <span className={AURORA_CARD_TITLE}>Could not load skills</span>
            </div>
            <p className={cn(AURORA_DENSE_META, 'mt-2')}>{state.detail}</p>
          </div>
        )}

        {state.kind === 'loaded' && state.rows.length === 0 && (
          <div className={cn(AURORA_MEDIUM_PANEL, 'mt-6 p-6')}>
            <span className={AURORA_CARD_TITLE}>No upstream is proxying skills</span>
            <p className={cn(AURORA_MUTED_LABEL, 'mt-2 max-w-2xl')}>
              Skills proxying is opt-in. Enable <code>proxy_skills</code> on an upstream to
              aggregate its Agent Skills through this gateway — an upstream&rsquo;s skills carry
              instructions an agent will act on, so it is a deliberate trust decision.
            </p>
          </div>
        )}

        {state.kind === 'loaded' && state.rows.length > 0 && (
          <>
            <div className={cn(AURORA_STRONG_PANEL, 'mt-6 flex items-baseline gap-3 p-6')}>
              <span className={AURORA_DISPLAY_NUMBER}>{totalSkillCount(state.rows)}</span>
              <span className={AURORA_MUTED_LABEL}>
                skills across {state.rows.length} upstream{state.rows.length === 1 ? '' : 's'}
              </span>
            </div>

            <div className="mt-4 flex flex-col gap-4">
              {state.rows.map((row) => {
                const status = skillsRowStatus(row)
                const { Icon, tone, label } = statusPresentation(status)
                return (
                  <div key={row.upstream} className={cn(AURORA_MEDIUM_PANEL, 'p-5')}>
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <div className="flex items-center gap-2">
                        <Icon className={cn('size-4', tone)} />
                        <span className={AURORA_CARD_TITLE}>{row.upstream}</span>
                        {!row.enabled && <Badge variant="outline">disabled</Badge>}
                        <Badge variant="secondary">{label}</Badge>
                      </div>
                      <span className={AURORA_DENSE_META}>
                        cached {formatCacheAge(row.cache_age_secs)}
                      </span>
                    </div>

                    <p className={cn(AURORA_DENSE_META, 'mt-2')}>{skillsRowSummary(row)}</p>

                    {row.skills.length > 0 && (
                      <ul className="mt-3 flex flex-col gap-2">
                        {row.skills.map((skill) => (
                          <li key={skill.uri} className="flex flex-col gap-0.5">
                            <div className="flex items-center gap-2">
                              <span className="font-medium">{skill.name}</span>
                              <span className={AURORA_DENSE_META}>
                                {skill.resource_count} file{skill.resource_count === 1 ? '' : 's'}
                              </span>
                            </div>
                            {skill.description && (
                              <span className={AURORA_MUTED_LABEL}>{skill.description}</span>
                            )}
                            <code className={AURORA_DENSE_META}>{skill.uri}</code>
                          </li>
                        ))}
                      </ul>
                    )}
                  </div>
                )
              })}
            </div>
          </>
        )}
      </div>
    </div>
  )
}
