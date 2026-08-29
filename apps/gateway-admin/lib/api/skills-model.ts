/**
 * Shaping for the `gateway.skills.list` operator view.
 *
 * The action returns one row per configured upstream. The panel needs two
 * things the raw rows do not give directly: a stable ordering, and a per-row
 * health read that says *why* a row looks the way it does — an upstream that
 * errored, one whose catalog was cut short by a budget, and one that simply has
 * no skills are three different situations and must not render identically.
 */

export interface UpstreamSkill {
  name: string
  uri: string
  description: string | null
  resource_count: number
  exposed: boolean
}

export interface UpstreamSkillRejection {
  uri: string
  reason: string
  detail?: string
}

export interface UpstreamSkillsRow {
  upstream: string
  enabled: boolean
  trusted: boolean
  supports_skills: boolean | null
  exposure_patterns: string[] | null
  skills: UpstreamSkill[]
  discovered_count: number
  exposed_count: number
  rejected: UpstreamSkillRejection[]
  excluded_count: number
  truncated: boolean
  cache_age_secs: number
  error: string | null
}

export type SkillsRowStatus =
  | 'error'
  | 'disabled'
  | 'unsupported'
  | 'unknown'
  | 'untrusted'
  | 'truncated'
  | 'excluded'
  | 'empty'
  | 'ok'

export type SkillsInventoryFilter = 'all' | 'attention' | 'ready' | 'not-participating'

export interface SkillsReadiness {
  participating: number
  ready: number
  needs_attention: number
  not_participating: number
}

export interface SkillsRejectionGroup {
  reason: string
  label: string
  guidance: string
  items: UpstreamSkillRejection[]
}

/**
 * The single most important thing to tell an operator about a row.
 *
 * Ordered by severity, not by field order: an upstream that failed outright is
 * reported as failed even if it also has stale counts, because the counts are
 * from a previous successful fetch and would otherwise read as current.
 */
export function skillsRowStatus(row: UpstreamSkillsRow): SkillsRowStatus {
  if (row.error) return 'error'
  if (!row.enabled) return 'disabled'
  if (row.supports_skills === false) return 'unsupported'
  if (row.supports_skills === null) return 'unknown'
  if (!row.trusted) return 'untrusted'
  if (row.truncated) return 'truncated'
  if (row.excluded_count > 0) return 'excluded'
  if (row.skills.length === 0) return 'empty'
  return 'ok'
}

/** Human-readable explanation for a row's status. */
export function skillsRowSummary(row: UpstreamSkillsRow): string {
  switch (skillsRowStatus(row)) {
    case 'error':
      return row.error ?? 'Unreachable'
    case 'disabled':
      return 'Server disabled'
    case 'unsupported':
      return 'Skills extension not advertised'
    case 'unknown':
      return 'Skills support has not been observed yet'
    case 'untrusted':
      return 'Skills supported, trust not enabled'
    case 'truncated':
      return `${row.discovered_count} discovered — the catalog was cut short by a size budget`
    case 'excluded':
      return `${row.discovered_count} discovered, ${row.excluded_count} rejected as unverifiable`
    case 'empty':
      // Not the same as "has no skills": the spec says an empty listing is
      // never proof of that, and an unlisted skill can still be fetched by URI.
      return 'No skills listed (a server may still serve skills by URI)'
    case 'ok':
      return `${row.exposed_count}/${row.discovered_count} exposed`
  }
}

/** Rows ordered so the ones needing attention sort first, then by name. */
export function sortSkillsRows(rows: UpstreamSkillsRow[]): UpstreamSkillsRow[] {
  const severity: Record<SkillsRowStatus, number> = {
    error: 0,
    disabled: 1,
    unsupported: 2,
    unknown: 3,
    untrusted: 4,
    truncated: 5,
    excluded: 6,
    empty: 7,
    ok: 8,
  }
  return [...rows].sort((a, b) => {
    const bySeverity = severity[skillsRowStatus(a)] - severity[skillsRowStatus(b)]
    return bySeverity !== 0 ? bySeverity : a.upstream.localeCompare(b.upstream)
  })
}

export function isSkillsParticipant(row: UpstreamSkillsRow): boolean {
  return row.enabled && row.supports_skills === true
}

export function skillsReadiness(rows: UpstreamSkillsRow[]): SkillsReadiness {
  return rows.reduce<SkillsReadiness>((result, row) => {
    if (!isSkillsParticipant(row)) {
      result.not_participating += 1
    } else if (skillsRowStatus(row) === 'ok' || skillsRowStatus(row) === 'empty') {
      result.participating += 1
      result.ready += 1
    } else {
      result.participating += 1
      result.needs_attention += 1
    }
    return result
  }, { participating: 0, ready: 0, needs_attention: 0, not_participating: 0 })
}

export function filterSkillsRows(
  rows: UpstreamSkillsRow[],
  query: string,
  filter: SkillsInventoryFilter,
): UpstreamSkillsRow[] {
  const needle = query.trim().toLocaleLowerCase()
  return rows.filter((row) => {
    const status = skillsRowStatus(row)
    const matchesFilter = filter === 'all'
      || (filter === 'attention' && isSkillsParticipant(row) && status !== 'ok' && status !== 'empty')
      || (filter === 'ready' && isSkillsParticipant(row) && (status === 'ok' || status === 'empty'))
      || (filter === 'not-participating' && !isSkillsParticipant(row))
    if (!matchesFilter) return false
    if (!needle) return true
    return [
      row.upstream,
      skillsRowSummary(row),
      ...row.skills.flatMap((skill) => [skill.name, skill.description ?? '', skill.uri]),
      ...row.rejected.flatMap((item) => [item.reason, item.detail ?? '', item.uri]),
    ].some((value) => value.toLocaleLowerCase().includes(needle))
  })
}

const REJECTION_HELP: Record<string, { label: string; guidance: string }> = {
  invalid_frontmatter: {
    label: 'Invalid manifest frontmatter',
    guidance: 'Update SKILL.md YAML to match SEP-2640: name and description are required strings; allowed-tools is a space-separated string; metadata values must be strings.',
  },
  invalid_skill_uri: {
    label: 'Invalid skill URI',
    guidance: 'Serve the manifest from a canonical skill resource URI ending in /SKILL.md.',
  },
  missing_manifest: {
    label: 'Manifest missing',
    guidance: 'Publish a readable SKILL.md resource for the advertised skill URI.',
  },
  invalid_digest: {
    label: 'Invalid resource digest',
    guidance: 'Publish a supported content digest for every manifest resource.',
  },
  manifest_uri_out_of_namespace: {
    label: 'Resource outside skill namespace',
    guidance: 'Keep every manifest URI within the advertised skill directory and origin.',
  },
  manifest_missing_skill_md: {
    label: 'SKILL.md missing from manifest',
    guidance: 'Include the skill URI itself in the manifest with the digest of SKILL.md.',
  },
  manifest_duplicate_uri: {
    label: 'Duplicate manifest resource',
    guidance: 'List every resource URI exactly once in the skill manifest.',
  },
  manifest_too_large: {
    label: 'Manifest resource limit exceeded',
    guidance: 'Reduce the skill package to at most 64 manifest resources.',
  },
}

export function groupSkillRejections(rejected: UpstreamSkillRejection[]): SkillsRejectionGroup[] {
  const groups = new Map<string, UpstreamSkillRejection[]>()
  for (const item of rejected) groups.set(item.reason, [...(groups.get(item.reason) ?? []), item])
  return [...groups.entries()]
    .map(([reason, items]) => ({
      reason,
      label: REJECTION_HELP[reason]?.label ?? reason.replaceAll('_', ' '),
      guidance: REJECTION_HELP[reason]?.guidance ?? 'Review the validation detail and correct the upstream manifest before refreshing.',
      items,
    }))
    .sort((a, b) => b.items.length - a.items.length || a.label.localeCompare(b.label))
}

/** Compact cache age for display. */
export function formatCacheAge(seconds: number): string {
  if (seconds <= 0) return 'just now'
  if (seconds < 60) return `${seconds}s ago`
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`
  return `${Math.floor(seconds / 3600)}h ago`
}

/** Total skills across every row, for the page header. */
export function totalSkillCount(rows: UpstreamSkillsRow[]): number {
  return rows.reduce((total, row) => total + row.discovered_count, 0)
}

export function totalExposedSkillCount(rows: UpstreamSkillsRow[]): number {
  return rows.reduce((total, row) => total + row.exposed_count, 0)
}
