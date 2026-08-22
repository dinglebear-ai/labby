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
