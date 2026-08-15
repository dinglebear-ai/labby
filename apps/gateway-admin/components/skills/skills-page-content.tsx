'use client'

import { useCallback, useEffect, useState } from 'react'
import {
  AlertTriangle,
  BookOpen,
  Cable,
  Info,
  Loader2,
  RefreshCw,
  Scissors,
  ShieldAlert,
} from 'lucide-react'

import { AppHeader } from '@/components/app-header'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { ConsoleHero, type ConsoleHeroStat } from '@/components/console/console-hero'
import { DashboardPanel } from '@/components/dashboard/panel'
import {
  AURORA_CARD_TITLE,
  AURORA_DENSE_META,
  AURORA_MUTED_LABEL,
  AURORA_PAGE_FRAME,
  AURORA_PAGE_SHELL,
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

  // Every hero number comes from `gateway.skills.list`. Nothing is derived that
  // the action does not report, so anything unknown stays an em dash rather
  // than a plausible-looking zero.
  const rows = state.kind === 'loaded' ? state.rows : null
  const excludedTotal = rows ? rows.reduce((sum, row) => sum + row.excluded_count, 0) : null
  const truncatedCount = rows ? rows.filter((row) => row.truncated).length : null
  const unreachableCount = rows ? rows.filter((row) => row.error !== null).length : null

  const stats: ConsoleHeroStat[] = [
    {
      label: 'Skills',
      value: rows ? totalSkillCount(rows) : '—',
      icon: <BookOpen size={12} strokeWidth={1.8} />,
    },
    {
      label: 'Upstreams',
      value: rows ? rows.length : '—',
      icon: <Cable size={12} strokeWidth={1.8} />,
    },
    {
      label: 'Excluded',
      value: excludedTotal ?? '—',
      icon: <AlertTriangle size={12} strokeWidth={1.8} />,
      tone: excludedTotal ? 'var(--aurora-warn)' : undefined,
    },
    {
      label: 'Truncated',
      value: truncatedCount ?? '—',
      icon: <Scissors size={12} strokeWidth={1.8} />,
      tone: truncatedCount ? 'var(--aurora-warn)' : undefined,
    },
    {
      label: 'Unreachable',
      value: unreachableCount ?? '—',
      icon: <ShieldAlert size={12} strokeWidth={1.8} />,
      tone: unreachableCount ? 'var(--aurora-error)' : undefined,
    },
  ]

  const pulse =
    rows === null || rows.length === 0
      ? undefined
      : unreachableCount
        ? {
            color: 'var(--aurora-error)',
            label: `${unreachableCount} unreachable`,
          }
        : excludedTotal || truncatedCount
          ? { color: 'var(--aurora-warn)', label: 'partial catalog' }
          : { color: 'var(--aurora-success)', label: 'all skills listed' }

  return (
    <>
      <AppHeader breadcrumbs={[{ label: 'Skills' }]} />
      <div className={`${AURORA_PAGE_SHELL} flex-1`}>
        <div className={AURORA_PAGE_FRAME}>
          {/* Hero — the mock's eyebrow + title + action cluster with the stat
              strip welded to the card's bottom edge, not floating cards. */}
          <ConsoleHero
            eyebrow="Catalog"
            pulse={pulse}
            title="Agent Skills"
            actions={
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
            }
            stats={stats}
          />

          <DashboardPanel title="Skills proxying" icon={<Info className="size-4" />}>
            <p className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>
              Skills aggregated from upstreams with <code>proxy_skills</code> enabled. Labby serves
              its own skills separately under the reserved <code>labby</code> origin.
            </p>
          </DashboardPanel>

          {state.kind === 'loading' && (
            <DashboardPanel title="Upstreams" icon={<Cable className="size-4" />}>
              <div className="flex items-center gap-3">
                <Loader2 className="size-4 animate-spin" />
                <span className={AURORA_MUTED_LABEL}>Loading skills…</span>
              </div>
            </DashboardPanel>
          )}

          {state.kind === 'error' && (
            <DashboardPanel
              title="Upstreams"
              icon={<ShieldAlert className="size-4 text-destructive" />}
            >
              <span className={AURORA_CARD_TITLE}>Could not load skills</span>
              <p className={AURORA_DENSE_META}>{state.detail}</p>
            </DashboardPanel>
          )}

          {state.kind === 'loaded' && state.rows.length === 0 && (
            <DashboardPanel title="Upstreams" icon={<Cable className="size-4" />}>
              <span className={AURORA_CARD_TITLE}>No upstream is proxying skills</span>
              <p className={cn(AURORA_DENSE_META, 'max-w-2xl text-aurora-text-muted')}>
                Skills proxying is opt-in. Enable <code>proxy_skills</code> on an upstream to
                aggregate its Agent Skills through this gateway — an upstream&rsquo;s skills carry
                instructions an agent will act on, so it is a deliberate trust decision.
              </p>
            </DashboardPanel>
          )}

          {state.kind === 'loaded' && state.rows.length > 0 && (
            <div
              style={{
                display: 'grid',
                gridTemplateColumns: 'repeat(auto-fit, minmax(320px, 1fr))',
                gap: 12,
                alignItems: 'start',
              }}
            >
              {state.rows.map((row) => {
                const status = skillsRowStatus(row)
                const { Icon, tone, label } = statusPresentation(status)
                return (
                  <DashboardPanel
                    key={row.upstream}
                    title={row.upstream}
                    icon={<Icon className={cn('size-4', tone)} />}
                    meta={`cached ${formatCacheAge(row.cache_age_secs)}`}
                  >
                    <div className="flex flex-wrap items-center gap-2">
                      {!row.enabled && <Badge variant="outline">disabled</Badge>}
                      <Badge variant="secondary">{label}</Badge>
                      <span className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>
                        {skillsRowSummary(row)}
                      </span>
                    </div>

                    {row.skills.length > 0 && (
                      <ul className="flex flex-col gap-2">
                        {row.skills.map((skill) => (
                          <li key={skill.uri} className="flex flex-col gap-0.5">
                            <div className="flex items-center gap-2">
                              <span className="font-medium">{skill.name}</span>
                              <span className={AURORA_DENSE_META}>
                                {skill.resource_count} file{skill.resource_count === 1 ? '' : 's'}
                              </span>
                            </div>
                            {skill.description && (
                              <span className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>
                                {skill.description}
                              </span>
                            )}
                            <code className={AURORA_DENSE_META}>{skill.uri}</code>
                          </li>
                        ))}
                      </ul>
                    )}
                  </DashboardPanel>
                )
              })}
            </div>
          )}
        </div>
      </div>
    </>
  )
}
