import test from 'node:test'
import assert from 'node:assert/strict'

import {
  formatCacheAge,
  filterSkillsRows,
  groupSkillRejections,
  skillsReadiness,
  skillsRowStatus,
  skillsRowSummary,
  sortSkillsRows,
  totalSkillCount,
  type UpstreamSkillsRow,
} from './skills-model.ts'

function row(partial: Partial<UpstreamSkillsRow> & Pick<UpstreamSkillsRow, 'upstream'>): UpstreamSkillsRow {
  const skills = partial.skills ?? []
  return {
    upstream: partial.upstream,
    enabled: partial.enabled ?? true,
    trusted: partial.trusted ?? true,
    supports_skills: Object.hasOwn(partial, 'supports_skills') ? partial.supports_skills! : true,
    exposure_patterns: partial.exposure_patterns ?? null,
    skills,
    discovered_count: partial.discovered_count ?? skills.length,
    exposed_count: partial.exposed_count ?? skills.filter((skill) => skill.exposed).length,
    rejected: partial.rejected ?? [],
    excluded_count: partial.excluded_count ?? 0,
    truncated: partial.truncated ?? false,
    cache_age_secs: partial.cache_age_secs ?? 0,
    error: partial.error ?? null,
  }
}

test('a cold untrusted upstream stays out of the actionable skills queue', () => {
  const cold = row({ upstream: 'cold', trusted: false, supports_skills: null })
  assert.equal(filterSkillsRows([cold], '', 'attention').length, 0)
  assert.equal(filterSkillsRows([cold], '', 'not-participating').length, 1)
})

const skill = { name: 'refunds', uri: 'skill://gh/refunds/SKILL.md', description: 'd', resource_count: 2, exposed: true }

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
  assert.match(skillsRowSummary(excluded), /2 rejected as unverifiable/)
})

test('an empty listing is not reported as proof of no skills', () => {
  // The spec is explicit that an empty or partial listing never proves absence,
  // and an unlisted skill can still be fetched by URI.
  const empty = row({ upstream: 'gh' })
  assert.equal(skillsRowStatus(empty), 'empty')
  assert.match(skillsRowSummary(empty), /may still serve skills by URI/)
})

test('a healthy row reports exposed versus discovered counts', () => {
  assert.equal(skillsRowSummary(row({ upstream: 'gh', skills: [skill] })), '1/1 exposed')
  assert.equal(skillsRowSummary(row({ upstream: 'gh', skills: [skill, skill] })), '2/2 exposed')
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

test('readiness separates actionable participants from fleet noise', () => {
  const rows = [
    row({ upstream: 'ready', skills: [skill] }),
    row({ upstream: 'review', trusted: false }),
    row({ upstream: 'disabled', enabled: false }),
    row({ upstream: 'legacy', supports_skills: false }),
  ]
  assert.deepEqual(skillsReadiness(rows), {
    participating: 2,
    ready: 1,
    needs_attention: 1,
    not_participating: 2,
  })
})

test('inventory filters by operator task and searches nested skill and rejection data', () => {
  const rows = [
    row({ upstream: 'ready', skills: [{ ...skill, description: 'Billing specialist' }] }),
    row({ upstream: 'review', trusted: false }),
    row({ upstream: 'broken', rejected: [{ uri: 'skill://broken/x/SKILL.md', reason: 'invalid_frontmatter', detail: 'description is required' }], excluded_count: 1 }),
    row({ upstream: 'legacy', supports_skills: false }),
  ]
  assert.deepEqual(filterSkillsRows(rows, '', 'attention').map((item) => item.upstream), ['review', 'broken'])
  assert.deepEqual(filterSkillsRows(rows, '', 'ready').map((item) => item.upstream), ['ready'])
  assert.deepEqual(filterSkillsRows(rows, '', 'not-participating').map((item) => item.upstream), ['legacy'])
  assert.deepEqual(filterSkillsRows(rows, 'billing', 'all').map((item) => item.upstream), ['ready'])
  assert.deepEqual(filterSkillsRows(rows, 'description is required', 'all').map((item) => item.upstream), ['broken'])
})

test('rejections are grouped into plain-language remediation buckets', () => {
  const groups = groupSkillRejections([
    { uri: 'skill://a/one/SKILL.md', reason: 'invalid_frontmatter', detail: 'bad metadata' },
    { uri: 'skill://a/two/SKILL.md', reason: 'invalid_frontmatter', detail: 'bad tools' },
    { uri: 'bad', reason: 'invalid_skill_uri' },
  ])
  assert.equal(groups[0]?.label, 'Invalid manifest frontmatter')
  assert.equal(groups[0]?.items.length, 2)
  assert.match(groups[0]?.guidance ?? '', /SEP-2640/)
  assert.equal(groups[1]?.label, 'Invalid skill URI')
})

test('every backend rejection code has specific remediation guidance', () => {
  const reasons = [
    'invalid_skill_uri',
    'invalid_frontmatter',
    'missing_manifest',
    'invalid_digest',
    'manifest_uri_out_of_namespace',
    'manifest_missing_skill_md',
    'manifest_duplicate_uri',
    'manifest_too_large',
  ]
  for (const reason of reasons) {
    const [group] = groupSkillRejections([{ reason, uri: 'skill://fixture/SKILL.md' }])
    assert.ok(group)
    assert.notEqual(group.label, reason.replaceAll('_', ' '))
    assert.notEqual(group.guidance, 'Review the validation detail and correct the upstream manifest before refreshing.')
  }
})
