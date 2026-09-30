import test from 'node:test'
import assert from 'node:assert/strict'
import { gatewaySkillDraft } from './gateway-skill-draft'
import type { Gateway } from '@/lib/types/gateway'

const gateway: Gateway = {
  id: 'gw-1', name: 'github-server', transport: 'http', configured: true, enabled: true,
  config: { url: 'https://example.test/mcp' },
  status: { healthy: true, connected: true, discovered_tool_count: 2, exposed_tool_count: 1, discovered_resource_count: 1, exposed_resource_count: 1, discovered_prompt_count: 1, exposed_prompt_count: 1 },
  discovery: {
    tools: [
      { name: 'github::search_issues', description: 'Search repository issues.', exposed: true, matched_by: null },
      { name: 'github::delete_repo', description: 'Delete a repository.', exposed: false, matched_by: null },
    ],
    resources: [{ name: 'Repository README', uri: 'repo://README.md', description: 'Project README.', exposed: true }],
    prompts: [{ name: 'review-pr', description: 'Review a pull request.', exposed: true }],
  },
  warnings: [],
}

test('gatewaySkillDraft derives a safe truthful Skill from exposed discovery data', () => {
  const draft = gatewaySkillDraft(gateway)
  assert.equal(draft.metadata.name, 'github-server-ops')
  assert.equal(draft.metadata.allowedTools, 'github::search_issues')
  assert.deepEqual(draft.metadata.tags, ['mcp', 'gateway', 'github-server'])
  assert.match(draft.content, /Github Server capabilities/)
  assert.match(draft.content, /github::search_issues/)
  assert.match(draft.content, /Repository README/)
  assert.match(draft.content, /review-pr/)
  assert.doesNotMatch(draft.content, /delete_repo/)
  assert.match(draft.content, /Generated from the server catalog currently visible to Labby/)
})
