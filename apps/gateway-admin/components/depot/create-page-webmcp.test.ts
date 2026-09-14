import assert from 'node:assert/strict'
import test from 'node:test'

import { validateArtifactDraft, type ArtifactMetadata } from '@/lib/editor/artifact-standards'
import {
  registerCreatePageWebMcpTools,
  type CreatePageDraftPatch,
  type CreatePageDraftSnapshot,
  type WebMcpModelContext,
  type WebMcpTool,
} from './create-page-webmcp'

const metadata: ArtifactMetadata = {
  name: 'repo-triage',
  description: 'Triage a repository.',
  tags: ['review'],
  license: '',
  compatibility: '',
  allowedTools: 'Read Grep',
}
const content = '## Workflow\n\nReview the repository.'

function snapshot(overrides: Partial<CreatePageDraftSnapshot> = {}): CreatePageDraftSnapshot {
  const kind = overrides.kind ?? 'Skill'
  const nextMetadata = overrides.metadata ?? metadata
  const nextContent = overrides.content ?? content
  return {
    kind,
    metadata: nextMetadata,
    content: nextContent,
    issues: overrides.issues ?? validateArtifactDraft(kind, nextMetadata, nextContent),
    canPublish: overrides.canPublish ?? true,
    publishReason: overrides.publishReason ?? null,
  }
}

function registerFixture(options: {
  current?: CreatePageDraftSnapshot
  configureDraft?: (patch: CreatePageDraftPatch) => CreatePageDraftSnapshot | Promise<CreatePageDraftSnapshot>
  publishSkill?: () => { jobId: string; status: string } | Promise<{ jobId: string; status: string }>
  waitForVisibleUpdate?: () => void | Promise<void>
  reportError?: (error: unknown) => void
} = {}) {
  const tools = new Map<string, { tool: WebMcpTool; signal?: AbortSignal }>()
  const controller = new AbortController()
  let current = options.current ?? snapshot()
  const context: WebMcpModelContext = {
    registerTool(tool, registration) {
      tools.set(tool.name, { tool, signal: registration?.signal })
    },
  }
  registerCreatePageWebMcpTools({
    context,
    signal: controller.signal,
    getDraft: () => current,
    configureDraft: options.configureDraft ?? ((patch) => {
      const nextKind = patch.kind ?? current.kind
      const nextMetadata = {
        ...current.metadata,
        ...(patch.name !== undefined ? { name: patch.name } : {}),
        ...(patch.description !== undefined ? { description: patch.description } : {}),
        ...(patch.tags !== undefined ? { tags: [...patch.tags] } : {}),
        ...(patch.license !== undefined ? { license: patch.license } : {}),
        ...(patch.compatibility !== undefined ? { compatibility: patch.compatibility } : {}),
        ...(patch.allowedTools !== undefined ? { allowedTools: patch.allowedTools } : {}),
      }
      const nextContent = patch.content ?? current.content
      current = snapshot({ kind: nextKind, metadata: nextMetadata, content: nextContent })
      return current
    }),
    publishSkill: options.publishSkill ?? (() => ({ jobId: 'job-123', status: 'queued' })),
    waitForVisibleUpdate: options.waitForVisibleUpdate ?? (() => {}),
    reportError: options.reportError,
  })
  return { tools, controller, getCurrent: () => current, setCurrent: (value: CreatePageDraftSnapshot) => { current = value } }
}

test('registers the Create page WebMCP tools with explicit action semantics and one lifecycle signal', () => {
  const { tools, controller } = registerFixture()
  assert.deepEqual([...tools.keys()], ['read_create_draft', 'configure_create_draft', 'complete_skill_publish'])

  assert.deepEqual(tools.get('read_create_draft')!.tool.annotations, { readOnlyHint: true, untrustedContentHint: true })
  assert.deepEqual(tools.get('configure_create_draft')!.tool.annotations, { readOnlyHint: false, untrustedContentHint: true })
  assert.deepEqual(tools.get('complete_skill_publish')!.tool.annotations, { readOnlyHint: false, untrustedContentHint: false })
  assert.ok([...tools.values()].every(({ signal }) => signal === controller.signal))
})

test('reads concise draft state and batches staged draft configuration before returning', async () => {
  let visibleUpdates = 0
  const fixture = registerFixture({ waitForVisibleUpdate: () => { visibleUpdates += 1 } })
  const readTool = fixture.tools.get('read_create_draft')!.tool
  const configureTool = fixture.tools.get('configure_create_draft')!.tool

  const initial = await readTool.execute({ includeContent: true, includeSource: true }) as Record<string, unknown>
  assert.equal(initial.kind, 'Skill')
  assert.equal(initial.path, 'skills/repo-triage/SKILL.md')
  assert.equal(initial.content, content)
  assert.match(String(initial.source), /^---\nname: "repo-triage"/)

  const configured = await configureTool.execute({
    name: 'release-triage',
    description: 'Triage a release.',
    tags: ['release', 'review'],
    content: '## Workflow\n\nReview the release.',
  }) as Record<string, unknown>
  assert.equal(visibleUpdates, 1)
  assert.equal(configured.path, 'skills/release-triage/SKILL.md')
  assert.equal(fixture.getCurrent().metadata.name, 'release-triage')
  assert.deepEqual(fixture.getCurrent().metadata.tags, ['release', 'review'])
  assert.equal(fixture.getCurrent().content, '## Workflow\n\nReview the release.')
})

test('rejects malformed staged inputs before changing the Create draft', async () => {
  const fixture = registerFixture()
  const configureTool = fixture.tools.get('configure_create_draft')!.tool

  await assert.rejects(async () => { await configureTool.execute({}) }, /requires at least one field/)
  await assert.rejects(async () => { await configureTool.execute({ kind: 'NotARealKind' }) }, /kind must be one of/)
  await assert.rejects(async () => { await configureTool.execute({ tags: ['ok', 12] }) }, /tags must be an array of strings/)
  await assert.rejects(async () => { await configureTool.execute({ name: 'next', surprise: true }) }, /does not accept/)
  assert.equal(fixture.getCurrent().metadata.name, 'repo-triage')
})

test('guards the publishing side effect with current name, validation, and availability', async () => {
  let publishCalls = 0
  const fixture = registerFixture({
    publishSkill: () => {
      publishCalls += 1
      return { jobId: 'job-456', status: 'accepted' }
    },
  })
  const publishTool = fixture.tools.get('complete_skill_publish')!.tool

  await assert.rejects(async () => { await publishTool.execute({ expectedName: 'stale-name' }) }, /draft name changed/)
  assert.equal(publishCalls, 0)

  fixture.setCurrent(snapshot({
    canPublish: false,
    publishReason: 'Publishing is disabled for this session.',
  }))
  await assert.rejects(async () => { await publishTool.execute({ expectedName: 'repo-triage' }) }, /disabled for this session/)
  assert.equal(publishCalls, 0)

  fixture.setCurrent(snapshot())
  const receipt = await publishTool.execute({ expectedName: 'repo-triage' }) as Record<string, unknown>
  assert.deepEqual(receipt, { name: 'repo-triage', jobId: 'job-456', status: 'accepted' })
  assert.equal(publishCalls, 1)
})

test('registration failures are reported without preventing later tools from registering', () => {
  const errors: unknown[] = []
  const names: string[] = []
  let attempts = 0
  const context: WebMcpModelContext = {
    registerTool(tool) {
      attempts += 1
      if (attempts === 1) throw new Error('registration failed')
      names.push(tool.name)
    },
  }
  registerCreatePageWebMcpTools({
    context,
    signal: new AbortController().signal,
    getDraft: () => snapshot(),
    configureDraft: () => snapshot(),
    publishSkill: () => ({ jobId: 'job', status: 'queued' }),
    reportError: (error) => errors.push(error),
  })

  assert.equal(errors.length, 1)
  assert.deepEqual(names, ['configure_create_draft', 'complete_skill_publish'])
})
