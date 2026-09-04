'use client'

import { useCallback, useEffect, useState } from 'react'
import { useRouter } from 'next/navigation'
import { toast } from 'sonner'
import {
  AlertTriangle,
  BookOpen,
  Cable,
  ChevronDown,
  Info,
  Loader2,
  RefreshCw,
  Scissors,
  ShieldAlert,
  Search,
} from 'lucide-react'

import { AppHeader } from '@/components/app-header'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ConsoleHero, type ConsoleHeroStat } from '@/components/console/console-hero'
import { DashboardPanel } from '@/components/dashboard/panel'
import { PrimitiveExposureTable } from '@/components/gateway/primitive-exposure-table'
import {
  AURORA_CARD_TITLE,
  AURORA_DENSE_META,
  AURORA_MUTED_LABEL,
  AURORA_PAGE_FRAME,
  AURORA_PAGE_SHELL,
} from '@/components/aurora/tokens'
import { GatewayApiError, gatewayAction } from '@/lib/api/gateway-client'
import { getMockSkillsRowsFallback } from '@/lib/api/mock-fallback'
import { useGatewayMutations } from '@/lib/hooks/use-gateways'
import {
  formatCacheAge,
  filterSkillsRows,
  groupSkillRejections,
  isSkillsParticipant,
  skillsReadiness,
  skillsRowStatus,
  skillsRowSummary,
  sortSkillsRows,
  totalExposedSkillCount,
  totalSkillCount,
  type SkillsRowStatus,
  type SkillsInventoryFilter,
  type UpstreamSkillsRow,
} from '@/lib/api/skills-model'
import { cn, getErrorMessage } from '@/lib/utils'

const USE_MOCK_DATA = process.env.NEXT_PUBLIC_MOCK_DATA === 'true'

type LoadState =
  | { kind: 'loading' }
  | { kind: 'loaded'; rows: UpstreamSkillsRow[] }
  | { kind: 'unavailable'; detail: string }
  | { kind: 'error'; detail: string }

export function RejectedSkillsList({ rejected }: { rejected: UpstreamSkillsRow['rejected'] }) {
  if (rejected.length === 0) return null
  const groups = groupSkillRejections(rejected)
  return (
    <DashboardPanel
      title="Rejected Skills"
      icon={<AlertTriangle className="size-4 text-amber-500" />}
      meta={`${rejected.length}`}
    >
      <div className="flex flex-col gap-3">
        <p className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>
          Labby validates manifests against the pinned SEP-2640 Agent Skills contract before exposure.
          Rejections are grouped by the upstream change needed.
        </p>
        {groups.map((group) => (
          <details key={group.reason} className="group rounded-lg border border-aurora-border-subtle bg-aurora-surface-muted/30 p-3" open={groups.length === 1}>
            <summary className="flex min-h-8 cursor-pointer list-none items-center gap-2 rounded focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary/40">
              <ChevronDown className="size-4 shrink-0 transition-transform group-open:rotate-180" />
              <span className="font-medium">{group.label}</span>
              <Badge variant="outline">{group.items.length}</Badge>
            </summary>
            <p className={cn(AURORA_DENSE_META, 'mt-2 text-aurora-text-muted')}>{group.guidance}</p>
            <ul className="mt-3 flex flex-col gap-2">
              {group.items.map((item) => (
                <li key={`${item.reason}:${item.uri}`} className="flex min-w-0 flex-col gap-0.5 border-l border-aurora-border-subtle pl-3">
                  <code className={cn(AURORA_DENSE_META, 'break-all')}>{item.uri}</code>
                  {item.detail ? <span className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>{item.detail}</span> : null}
                </li>
              ))}
            </ul>
          </details>
        ))}
      </div>
    </DashboardPanel>
  )
}

/** Icon and tone per row status, so severity reads before the text does. */
function statusPresentation(status: SkillsRowStatus) {
  switch (status) {
    case 'error':
      return { Icon: ShieldAlert, tone: 'text-destructive', label: 'Unreachable' }
    case 'disabled':
      return { Icon: AlertTriangle, tone: 'text-muted-foreground', label: 'Disabled' }
    case 'unsupported':
      return { Icon: Cable, tone: 'text-muted-foreground', label: 'Not advertised' }
    case 'unknown':
      return { Icon: Info, tone: 'text-amber-500', label: 'Support unknown' }
    case 'untrusted':
      return { Icon: ShieldAlert, tone: 'text-amber-500', label: 'Not trusted' }
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

export function SkillsPageContent({ upstream, embedded = false }: { upstream?: string; embedded?: boolean }) {
  const router = useRouter()
  const { updateGateway } = useGatewayMutations()
  const [state, setState] = useState<LoadState>({ kind: 'loading' })
  const [query, setQuery] = useState('')
  const [inventoryFilter, setInventoryFilter] = useState<SkillsInventoryFilter>('attention')
  const [showNonParticipants, setShowNonParticipants] = useState(false)

  const load = useCallback(async (signal?: AbortSignal) => {
    setState({ kind: 'loading' })
    try {
      const rows = USE_MOCK_DATA
        ? getMockSkillsRowsFallback(upstream)
        : await gatewayAction<UpstreamSkillsRow[]>(
            'gateway.skills.list',
            upstream ? { upstream } : {},
            signal,
          )
      setState({ kind: 'loaded', rows: sortSkillsRows(rows ?? []) })
    } catch (error) {
      // An aborted request is a navigation, not a failure to report.
      if (signal?.aborted) return
      const detail = error instanceof Error ? error.message : String(error)
      if (error instanceof GatewayApiError && error.code === 'feature_not_compiled') {
        setState({ kind: 'unavailable', detail })
        return
      }
      setState({ kind: 'error', detail })
    }
  }, [upstream])

  useEffect(() => {
    const controller = new AbortController()
    void load(controller.signal)
    return () => controller.abort()
  }, [load])

  // Every hero number comes from `gateway.skills.list`. Nothing is derived that
  // the action does not report, so anything unknown stays an em dash rather
  // than a plausible-looking zero.
  const rows = state.kind === 'loaded' ? state.rows : null
  const rejectedTotal = rows ? rows.reduce((sum, row) => sum + row.excluded_count, 0) : null
  const truncatedCount = rows ? rows.filter((row) => row.truncated).length : null
  const unreachableCount = rows ? rows.filter((row) => row.error !== null).length : null
  const supportedCount = rows ? rows.filter((row) => row.supports_skills === true).length : null
  const trustedCount = rows ? rows.filter((row) => row.trusted).length : null
  const readiness = rows ? skillsReadiness(rows) : null

  const stats: ConsoleHeroStat[] = [
    {
      label: 'Discovered',
      value: rows ? totalSkillCount(rows) : '—',
      icon: <BookOpen size={12} strokeWidth={1.8} />,
    },
    {
      label: 'Exposed',
      value: rows ? totalExposedSkillCount(rows) : '—',
      icon: <BookOpen size={12} strokeWidth={1.8} />,
    },
    {
      label: 'Supported',
      value: supportedCount ?? '—',
      icon: <Cable size={12} strokeWidth={1.8} />,
    },
    {
      label: 'Trusted',
      value: trustedCount ?? '—',
      icon: <ShieldAlert size={12} strokeWidth={1.8} />,
    },
    {
      label: 'Rejected',
      value: rejectedTotal ?? '—',
      icon: <AlertTriangle size={12} strokeWidth={1.8} />,
      tone: rejectedTotal ? 'var(--aurora-warn)' : undefined,
    },
  ]

  const selectedRow = upstream && rows?.length ? rows[0] : null
  const gatewayIdForUpstream = (name: string) => name

  const pulse =
    rows === null || rows.length === 0
      ? undefined
      : unreachableCount
        ? {
            color: 'var(--aurora-error)',
            label: `${unreachableCount} unreachable`,
          }
        : rejectedTotal || truncatedCount || (supportedCount ?? 0) > (trustedCount ?? 0)
          ? { color: 'var(--aurora-warn)', label: 'skills need review' }
          : { color: 'var(--aurora-success)', label: 'skills policy healthy' }

  return (
    <>
      {!embedded ? <AppHeader
        breadcrumbs={
          upstream
            ? [{ label: 'Skills', href: '/skills' }, { label: upstream }]
            : [{ label: 'Skills' }]
        }
      /> : null}
      <div className={`${AURORA_PAGE_SHELL} flex-1`}>
        <div className={AURORA_PAGE_FRAME}>
          {/* Hero — the mock's eyebrow + title + action cluster with the stat
              strip welded to the card's bottom edge, not floating cards. */}
          <ConsoleHero
            eyebrow={upstream ? "Upstream Skills" : "Catalog"}
            pulse={pulse}
            title={upstream ?? "Agent Skills"}
            actions={
              <div className="flex items-center gap-2">
                {upstream ? (
                  <Button variant="outline" size="sm" onClick={() => router.push('/skills')}>
                    All skills
                  </Button>
                ) : null}
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
            }
            stats={stats}
          />

          <DashboardPanel title="Skills trust and exposure" icon={<Info className="size-4" />}>
            <div className="grid gap-4 md:grid-cols-[minmax(0,1fr)_auto] md:items-center">
              <div>
                <span className={AURORA_CARD_TITLE}>Discover safely, then choose what agents can use</span>
                <p className={cn(AURORA_DENSE_META, 'mt-1 max-w-3xl text-aurora-text-muted')}>
                  Advertising the extension does not make its instructions trusted. Labby only reads catalogs
                  from servers with <code>proxy_skills</code> enabled, validates every manifest against the pinned
                  SEP-2640 contract, and republishes only the selection allowed by <code>expose_skills</code>.
                </p>
              </div>
              {readiness ? (
                <div className="flex flex-wrap gap-2 md:justify-end" aria-label="Skills readiness">
                  <Badge variant="secondary">{readiness.ready} ready</Badge>
                  {readiness.needs_attention > 0 ? <Badge variant="outline">{readiness.needs_attention} need action</Badge> : null}
                  <Badge variant="outline">{readiness.not_participating} not participating</Badge>
                </div>
              ) : null}
            </div>
          </DashboardPanel>

          {state.kind === 'loading' && (
            <DashboardPanel title="Upstreams" icon={<Cable className="size-4" />}>
              <div className="flex items-center gap-3">
                <Loader2 className="size-4 animate-spin" />
                <span className={AURORA_MUTED_LABEL}>Loading skills…</span>
              </div>
            </DashboardPanel>
          )}

          {state.kind === 'unavailable' && (
            <DashboardPanel
              title="Skills unavailable"
              icon={<AlertTriangle className="size-4 text-amber-500" />}
            >
              <span className={AURORA_CARD_TITLE}>This Labby build does not include Agent Skills</span>
              <p className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>
                Release builds include Skills. Rebuild this development binary with the <code>skills</code> feature
                to enable this surface.
              </p>
              <p className={AURORA_DENSE_META}>{state.detail}</p>
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
              <span className={AURORA_CARD_TITLE}>
                {upstream ? `No gateway named ${upstream}` : 'No upstream gateways configured'}
              </span>
              <p className={cn(AURORA_DENSE_META, 'max-w-2xl text-aurora-text-muted')}>
                {upstream
                  ? 'Return to the full Skills catalog and choose another gateway.'
                  : 'Add an MCP upstream to inspect whether it advertises Agent Skills.'}
              </p>
            </DashboardPanel>
          )}

          {state.kind === 'loaded' && upstream && selectedRow && (
            <>
              {(() => {
                const status = skillsRowStatus(selectedRow)
                const { Icon, tone, label } = statusPresentation(status)
                return (
                  <DashboardPanel
                    title="Upstream status"
                    icon={<Icon className={cn('size-4', tone)} />}
                    action={
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() =>
                          router.push(`/gateway?id=${encodeURIComponent(gatewayIdForUpstream(selectedRow.upstream))}`)
                        }
                      >
                        Server settings
                      </Button>
                    }
                  >
                    <div className="flex flex-wrap items-center gap-2">
                      <Badge variant="secondary">{label}</Badge>
                      <Badge variant="outline">
                        {selectedRow.supports_skills === true
                          ? 'extension supported'
                          : selectedRow.supports_skills === false
                            ? 'not advertised'
                            : 'support unknown'}
                      </Badge>
                      <Badge variant={selectedRow.trusted ? 'secondary' : 'outline'}>
                        {selectedRow.trusted ? 'trusted' : 'not trusted'}
                      </Badge>
                      <span className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>
                        {skillsRowSummary(selectedRow)}
                      </span>
                    </div>
                  </DashboardPanel>
                )
              })()}

              {selectedRow.trusted && selectedRow.supports_skills !== false ? (
                <PrimitiveExposureTable
                  title="Discovered Agent Skills"
                  description="Search and manage which validated upstream skills are exposed through this gateway."
                  searchPlaceholder="Search skills"
                  manageLabel="Manage skills"
                  emptyLabel="No skills listed"
                  exposureEnabled={selectedRow.trusted}
                  icon={BookOpen}
                  items={selectedRow.skills.map((skill) => ({
                    name: skill.name,
                    description: skill.description ?? undefined,
                    secondary: `${skill.resource_count} file${skill.resource_count === 1 ? '' : 's'} · ${skill.uri}`,
                    exposed: skill.exposed,
                  }))}
                  onSaveSelection={async (selectedNames) => {
                    try {
                      await updateGateway(gatewayIdForUpstream(selectedRow.upstream), {
                        config: { expose_skills: selectedNames },
                      })
                      toast.success('Skill exposure updated.')
                      await load()
                    } catch (error) {
                      toast.error(getErrorMessage(error, 'Failed to update skill exposure'))
                      throw error
                    }
                  }}
                />
              ) : (
                <DashboardPanel title="Skill exposure" icon={<BookOpen className="size-4" />}>
                  <span className={AURORA_CARD_TITLE}>
                    {selectedRow.supports_skills === false
                      ? 'This upstream does not advertise Agent Skills'
                      : 'Trust Agent Skills before loading their instructions'}
                  </span>
                  <p className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>
                    {selectedRow.supports_skills === false
                      ? 'No skill catalog will be requested from this server.'
                      : 'The initialize handshake can advertise Skills without Labby executing or enumerating them. Enable trust in Server settings to load and manage the catalog.'}
                  </p>
                </DashboardPanel>
              )}

              <RejectedSkillsList rejected={selectedRow.rejected} />
            </>
          )}

          {state.kind === 'loaded' && !upstream && state.rows.length > 0 && (
            <DashboardPanel title="Operator inventory" icon={<BookOpen className="size-4" />} meta={`${readiness?.participating ?? 0} participating`}>
              <div className="flex flex-col gap-4">
                <div className="flex flex-col gap-2 lg:flex-row lg:items-center lg:justify-between">
                  <div className="relative w-full lg:max-w-md">
                    <Search className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-aurora-text-muted" />
                    <Input aria-label="Search skills inventory" value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search servers, skills, URIs, or validation errors" className="pl-9" />
                  </div>
                  <div className="flex flex-wrap gap-1" role="group" aria-label="Filter skills inventory">
                    {([
                      ['attention', `Needs action ${readiness?.needs_attention ?? 0}`],
                      ['ready', `Ready ${readiness?.ready ?? 0}`],
                      ['all', `All participants ${readiness?.participating ?? 0}`],
                    ] as const).map(([value, label]) => (
                      <Button key={value} size="sm" variant={inventoryFilter === value ? 'secondary' : 'ghost'} aria-pressed={inventoryFilter === value} onClick={() => setInventoryFilter(value)}>{label}</Button>
                    ))}
                  </div>
                </div>

                {filterSkillsRows(state.rows.filter(isSkillsParticipant), query, inventoryFilter === 'not-participating' ? 'all' : inventoryFilter).length === 0 ? (
                  <div className="rounded-lg border border-dashed border-aurora-border-subtle px-4 py-8 text-center">
                    <p className="font-medium">No participating servers match this view</p>
                    <p className={cn(AURORA_DENSE_META, 'mt-1 text-aurora-text-muted')}>Try another filter or search term.</p>
                  </div>
                ) : (
                  <div className="grid items-start gap-3 xl:grid-cols-2">
              {filterSkillsRows(state.rows.filter(isSkillsParticipant), query, inventoryFilter === 'not-participating' ? 'all' : inventoryFilter).map((row) => {
                const status = skillsRowStatus(row)
                const { Icon, tone, label } = statusPresentation(status)
                return (
                  <DashboardPanel
                    key={row.upstream}
                    title={row.upstream}
                    icon={<Icon className={cn('size-4', tone)} />}
                    meta={row.trusted ? `cached ${formatCacheAge(row.cache_age_secs)}` : undefined}
                    action={
                      <Button
                        variant={status === 'ok' || status === 'empty' ? 'outline' : 'secondary'}
                        size="sm"
                        onClick={() => router.push(`/skills?upstream=${encodeURIComponent(row.upstream)}`)}
                      >
                        {status === 'ok' || status === 'empty' ? 'Manage exposure' : 'Review'}
                      </Button>
                    }
                  >
                    <div className="flex flex-wrap items-center gap-2">
                      <Badge variant="secondary">{label}</Badge>
                      <Badge variant="outline">
                        {row.supports_skills === true
                          ? 'supported'
                          : row.supports_skills === false
                            ? 'not advertised'
                            : 'unknown'}
                      </Badge>
                      <Badge variant={row.trusted ? 'secondary' : 'outline'}>
                        {row.trusted ? 'trusted' : 'not trusted'}
                      </Badge>
                      <span className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>
                        {skillsRowSummary(row)}
                      </span>
                    </div>

                    {row.skills.length > 0 && (
                      <ul className="flex flex-col gap-2">
                        {row.skills.slice(0, 3).map((skill) => (
                          <li key={skill.uri} className="flex flex-col gap-0.5">
                            <div className="flex items-center gap-2">
                              <span className="font-medium">{skill.name}</span>
                              <Badge variant={skill.exposed ? 'secondary' : 'outline'}>
                                {skill.exposed ? 'exposed' : 'hidden'}
                              </Badge>
                            </div>
                            {skill.description && (
                              <span className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>
                                {skill.description}
                              </span>
                            )}
                          </li>
                        ))}
                        {row.skills.length > 3 && (
                          <li className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>
                            +{row.skills.length - 3} more
                          </li>
                        )}
                      </ul>
                    )}
                  </DashboardPanel>
                )
              })}
                  </div>
                )}

                {(readiness?.not_participating ?? 0) > 0 ? (
                  <div className="border-t border-aurora-border-subtle pt-3">
                    <Button variant="ghost" size="sm" aria-expanded={showNonParticipants} onClick={() => setShowNonParticipants((value) => !value)}>
                      <ChevronDown className={cn('size-4 transition-transform', showNonParticipants && 'rotate-180')} />
                      {showNonParticipants ? 'Hide' : 'Show'} {readiness?.not_participating} nonparticipating servers
                    </Button>
                    {showNonParticipants ? (
                      <div className="mt-3 grid items-start gap-3 xl:grid-cols-2">
                        {filterSkillsRows(state.rows, query, 'not-participating').map((row) => {
                          const status = skillsRowStatus(row)
                          const { Icon, tone, label } = statusPresentation(status)
                          return (
                            <div key={row.upstream} className="flex min-w-0 flex-col gap-2 rounded-lg border border-aurora-border-subtle bg-aurora-surface-muted/20 p-3 sm:flex-row sm:items-center sm:justify-between">
                              <div className="flex min-w-0 items-start gap-3">
                                <Icon className={cn('mt-0.5 size-4 shrink-0', tone)} />
                                <div className="min-w-0">
                                  <div className="flex flex-wrap items-center gap-2"><span className="font-medium">{row.upstream}</span><Badge variant="outline">{label}</Badge></div>
                                  <p className={cn(AURORA_DENSE_META, 'mt-1 text-aurora-text-muted')}>{skillsRowSummary(row)}</p>
                                </div>
                              </div>
                              <Button variant="outline" size="sm" onClick={() => router.push(`/skills?upstream=${encodeURIComponent(row.upstream)}`)}>Details</Button>
                            </div>
                          )
                        })}
                      </div>
                    ) : null}
                  </div>
                ) : null}
              </div>
            </DashboardPanel>
          )}
        </div>
      </div>
    </>
  )
}
