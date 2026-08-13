/**
 * Shaping for the `gateway.skills.list` operator view.
 *
 * The action returns one row per skills-proxying upstream. The panel needs two
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
}

export interface UpstreamSkillsRow {
  upstream: string
  enabled: boolean
  skills: UpstreamSkill[]
  excluded_count: number
  truncated: boolean
  cache_age_secs: number
  error: string | null
}

export type SkillsRowStatus = 'error' | 'truncated' | 'excluded' | 'empty' | 'ok'

/**
 * The single most important thing to tell an operator about a row.
 *
 * Ordered by severity, not by field order: an upstream that failed outright is
 * reported as failed even if it also has stale counts, because the counts are
 * from a previous successful fetch and would otherwise read as current.
 */
export function skillsRowStatus(row: UpstreamSkillsRow): SkillsRowStatus {
  if (row.error) return 'error'
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
    case 'truncated':
      return `${row.skills.length} shown — the catalog was cut short by a size budget`
    case 'excluded':
      return `${row.skills.length} shown, ${row.excluded_count} excluded as unverifiable`
    case 'empty':
      // Not the same as "has no skills": the spec says an empty listing is
      // never proof of that, and an unlisted skill can still be fetched by URI.
      return 'No skills listed (a server may still serve skills by URI)'
    case 'ok':
      return `${row.skills.length} skill${row.skills.length === 1 ? '' : 's'}`
  }
}

/** Rows ordered so the ones needing attention sort first, then by name. */
export function sortSkillsRows(rows: UpstreamSkillsRow[]): UpstreamSkillsRow[] {
  const severity: Record<SkillsRowStatus, number> = {
    error: 0,
    truncated: 1,
    excluded: 2,
    empty: 3,
    ok: 4,
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
  return rows.reduce((total, row) => total + row.skills.length, 0)
}
