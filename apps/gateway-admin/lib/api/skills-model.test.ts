import test from 'node:test'
import assert from 'node:assert/strict'

import {
  formatCacheAge,
  skillsRowStatus,
  skillsRowSummary,
  sortSkillsRows,
  totalSkillCount,
  type UpstreamSkillsRow,
} from './skills-model.ts'

function row(partial: Partial<UpstreamSkillsRow> & Pick<UpstreamSkillsRow, 'upstream'>): UpstreamSkillsRow {
  return {
    upstream: partial.upstream,
    enabled: partial.enabled ?? true,
    skills: partial.skills ?? [],
    excluded_count: partial.excluded_count ?? 0,
    truncated: partial.truncated ?? false,
    cache_age_secs: partial.cache_age_secs ?? 0,
    error: partial.error ?? null,
  }
}

const skill = { name: 'refunds', uri: 'skill://gh/refunds/SKILL.md', description: 'd', resource_count: 2 }

test('an errored row reports the error rather than its stale counts', () => {
  // The counts come from a previous successful fetch; rendering them as current
  // would tell an operator the upstream is fine.
  const errored = row({ upstream: 'gh', skills: [skill], excluded_count: 3, error: 'connection refused' })
  assert.equal(skillsRowStatus(errored), 'error')
  assert.equal(skillsRowSummary(errored), 'connection refused')
})

test('truncation outranks exclusions', () => {
  // Both are true, but truncation means the catalog is incomplete for a
  // different reason and is the more misleading one to hide.
  const both = row({ upstream: 'gh', skills: [skill], excluded_count: 2, truncated: true })
  assert.equal(skillsRowStatus(both), 'truncated')
  assert.match(skillsRowSummary(both), /cut short/)
})

test('exclusions are surfaced when the catalog is otherwise complete', () => {
  const excluded = row({ upstream: 'gh', skills: [skill], excluded_count: 2 })
  assert.equal(skillsRowStatus(excluded), 'excluded')
  assert.match(skillsRowSummary(excluded), /2 excluded/)
})

test('an empty listing is not reported as proof of no skills', () => {
  // The spec is explicit that an empty or partial listing never proves absence,
  // and an unlisted skill can still be fetched by URI.
  const empty = row({ upstream: 'gh' })
  assert.equal(skillsRowStatus(empty), 'empty')
  assert.match(skillsRowSummary(empty), /may still serve skills by URI/)
})

test('a healthy row pluralises honestly', () => {
  assert.equal(skillsRowSummary(row({ upstream: 'gh', skills: [skill] })), '1 skill')
  assert.equal(skillsRowSummary(row({ upstream: 'gh', skills: [skill, skill] })), '2 skills')
})

test('rows needing attention sort first, then alphabetically', () => {
  const rows = [
    row({ upstream: 'zeta', skills: [skill] }),
    row({ upstream: 'alpha', skills: [skill] }),
    row({ upstream: 'broken', error: 'refused' }),
    row({ upstream: 'cut', skills: [skill], truncated: true }),
  ]
  assert.deepEqual(
    sortSkillsRows(rows).map((entry) => entry.upstream),
    ['broken', 'cut', 'alpha', 'zeta'],
  )
})

test('sorting does not mutate the input', () => {
  const rows = [row({ upstream: 'zeta' }), row({ upstream: 'alpha' })]
  sortSkillsRows(rows)
  assert.deepEqual(rows.map((entry) => entry.upstream), ['zeta', 'alpha'])
})

test('cache age reads naturally across the ranges', () => {
  assert.equal(formatCacheAge(0), 'just now')
  assert.equal(formatCacheAge(45), '45s ago')
  assert.equal(formatCacheAge(90), '1m ago')
  assert.equal(formatCacheAge(7200), '2h ago')
})

test('the header total counts skills across every upstream', () => {
  const rows = [row({ upstream: 'a', skills: [skill, skill] }), row({ upstream: 'b', skills: [skill] })]
  assert.equal(totalSkillCount(rows), 3)
})
