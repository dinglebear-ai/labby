import {
  ARTIFACT_KINDS,
  artifactPath,
  composeArtifactSource,
  type ArtifactIssue,
  type ArtifactKind,
  type ArtifactMetadata,
} from '@/lib/editor/artifact-standards'

export interface CreatePageDraftSnapshot {
  kind: ArtifactKind
  metadata: ArtifactMetadata
  content: string
  issues: ArtifactIssue[]
  canPublish: boolean
  publishReason?: string | null
}

export interface CreatePageDraftPatch {
  kind?: ArtifactKind
  name?: string
  description?: string
  tags?: string[]
  license?: string
  compatibility?: string
  allowedTools?: string
  content?: string
}

export interface CreatePagePublishReceipt {
  jobId: string
  status: string
}

export interface WebMcpTool {
  name: string
  title?: string
  description: string
  inputSchema: object
  annotations?: {
    readOnlyHint?: boolean
    untrustedContentHint?: boolean
  }
  execute(input: unknown): unknown | Promise<unknown>
}

export interface WebMcpModelContext {
  registerTool(tool: WebMcpTool, options?: { signal?: AbortSignal }): void | Promise<void>
}

interface RegisterCreatePageWebMcpToolsOptions {
  context: WebMcpModelContext
  signal: AbortSignal
  getDraft: () => CreatePageDraftSnapshot
  configureDraft: (patch: CreatePageDraftPatch) => CreatePageDraftSnapshot | Promise<CreatePageDraftSnapshot>
  publishSkill: () => CreatePagePublishReceipt | Promise<CreatePagePublishReceipt>
  waitForVisibleUpdate?: () => void | Promise<void>
  reportError?: (error: unknown) => void
}

const DRAFT_PATCH_KEYS = new Set([
  'kind', 'name', 'description', 'tags', 'license', 'compatibility', 'allowedTools', 'content',
])
const READ_KEYS = new Set(['includeContent', 'includeSource'])
const PUBLISH_KEYS = new Set(['expectedName'])

function objectInput(input: unknown, toolName: string): Record<string, unknown> {
  if (input == null) return {}
  if (typeof input !== 'object' || Array.isArray(input)) throw new TypeError(toolName + ' input must be an object.')
  return input as Record<string, unknown>
}

function assertOnlyKeys(input: Record<string, unknown>, keys: ReadonlySet<string>, toolName: string) {
  const unknownKey = Object.keys(input).find((key) => !keys.has(key))
  if (unknownKey) throw new TypeError(toolName + ' does not accept "' + unknownKey + '".')
}

function optionalBoolean(input: Record<string, unknown>, key: string, toolName: string): boolean {
  const value = input[key]
  if (value === undefined) return false
  if (typeof value !== 'boolean') throw new TypeError(toolName + '.' + key + ' must be a boolean.')
  return value
}

function optionalString(input: Record<string, unknown>, key: string, toolName: string): string | undefined {
  const value = input[key]
  if (value === undefined) return undefined
  if (typeof value !== 'string') throw new TypeError(toolName + '.' + key + ' must be a string.')
  return value
}

function parseDraftPatch(input: unknown): CreatePageDraftPatch {
  const toolName = 'configure_create_draft'
  const value = objectInput(input, toolName)
  assertOnlyKeys(value, DRAFT_PATCH_KEYS, toolName)
  if (!Object.keys(value).length) throw new TypeError(toolName + ' requires at least one field to change.')

  let kind: ArtifactKind | undefined
  if (value.kind !== undefined) {
    if (typeof value.kind !== 'string' || !ARTIFACT_KINDS.includes(value.kind as ArtifactKind)) {
      throw new TypeError(toolName + '.kind must be one of: ' + ARTIFACT_KINDS.join(', ') + '.')
    }
    kind = value.kind as ArtifactKind
  }

  let tags: string[] | undefined
  if (value.tags !== undefined) {
    if (!Array.isArray(value.tags) || value.tags.some((tag) => typeof tag !== 'string')) {
      throw new TypeError(toolName + '.tags must be an array of strings.')
    }
    tags = [...value.tags]
  }

  return {
    kind,
    name: optionalString(value, 'name', toolName),
    description: optionalString(value, 'description', toolName),
    tags,
    license: optionalString(value, 'license', toolName),
    compatibility: optionalString(value, 'compatibility', toolName),
    allowedTools: optionalString(value, 'allowedTools', toolName),
    content: optionalString(value, 'content', toolName),
  }
}

function validationResult(issues: ArtifactIssue[]) {
  const simplify = (issue: ArtifactIssue) => ({ field: issue.field, message: issue.message })
  return {
    errors: issues.filter((issue) => issue.severity === 'error').map(simplify),
    warnings: issues.filter((issue) => issue.severity === 'warning').map(simplify),
  }
}

function draftResult(snapshot: CreatePageDraftSnapshot, includeContent = false, includeSource = false) {
  const result: Record<string, unknown> = {
    kind: snapshot.kind,
    path: artifactPath(snapshot.kind, snapshot.metadata.name),
    metadata: snapshot.metadata,
    validation: validationResult(snapshot.issues),
    publish: {
      available: snapshot.canPublish,
      reason: snapshot.publishReason ?? null,
    },
  }
  if (includeContent) result.content = snapshot.content
  if (includeSource) result.source = composeArtifactSource(snapshot.kind, snapshot.metadata, snapshot.content)
  return result
}

export function browserModelContext(): WebMcpModelContext | undefined {
  if (typeof document === 'undefined') return undefined
  return (document as Document & { modelContext?: WebMcpModelContext }).modelContext
}

export async function waitForBrowserPaint(): Promise<void> {
  if (typeof requestAnimationFrame !== 'function') {
    await Promise.resolve()
    return
  }
  await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
}

export function registerCreatePageWebMcpTools({
  context,
  signal,
  getDraft,
  configureDraft,
  publishSkill,
  waitForVisibleUpdate = waitForBrowserPaint,
  reportError = console.error,
}: RegisterCreatePageWebMcpToolsOptions): void {
  const register = (tool: WebMcpTool) => {
    try {
      void Promise.resolve(context.registerTool(tool, { signal })).catch(reportError)
    } catch (error) {
      reportError(error)
    }
  }

  register({
    name: 'read_create_draft',
    title: 'Read Create draft',
    description: 'Read the current Labby Create draft, validation state, and publish availability without changing it. Request content or composed source only when needed.',
    inputSchema: {
      type: 'object',
      properties: {
        includeContent: { type: 'boolean' },
        includeSource: { type: 'boolean' },
      },
      additionalProperties: false,
    },
    annotations: { readOnlyHint: true, untrustedContentHint: true },
    execute(input) {
      const toolName = 'read_create_draft'
      const value = objectInput(input, toolName)
      assertOnlyKeys(value, READ_KEYS, toolName)
      return draftResult(
        getDraft(),
        optionalBoolean(value, 'includeContent', toolName),
        optionalBoolean(value, 'includeSource', toolName),
      )
    },
  })

  register({
    name: 'configure_create_draft',
    title: 'Configure Create draft',
    description: 'Stage one or more changes in the local Labby Create draft and update the visible editor. This does not publish or save externally.',
    inputSchema: {
      type: 'object',
      properties: {
        kind: { type: 'string', enum: [...ARTIFACT_KINDS] },
        name: { type: 'string' },
        description: { type: 'string' },
        tags: { type: 'array', items: { type: 'string' } },
        license: { type: 'string' },
        compatibility: { type: 'string' },
        allowedTools: { type: 'string' },
        content: { type: 'string' },
      },
      additionalProperties: false,
    },
    annotations: { readOnlyHint: false, untrustedContentHint: true },
    async execute(input) {
      const snapshot = await configureDraft(parseDraftPatch(input))
      await waitForVisibleUpdate()
      return draftResult(snapshot)
    },
  })

  register({
    name: 'complete_skill_publish',
    title: 'Publish Skill draft',
    description: 'Publish the current valid Skill draft to Team Depot. This is an external side effect. expectedName must exactly match the visible draft name.',
    inputSchema: {
      type: 'object',
      properties: { expectedName: { type: 'string' } },
      required: ['expectedName'],
      additionalProperties: false,
    },
    annotations: { readOnlyHint: false, untrustedContentHint: false },
    async execute(input) {
      const toolName = 'complete_skill_publish'
      const value = objectInput(input, toolName)
      assertOnlyKeys(value, PUBLISH_KEYS, toolName)
      const expectedName = optionalString(value, 'expectedName', toolName)
      if (!expectedName) throw new TypeError(toolName + '.expectedName is required.')

      const draft = getDraft()
      if (draft.kind !== 'Skill') throw new Error('Only Skill drafts can be published from the Create page.')
      if (draft.metadata.name !== expectedName) throw new Error('The visible draft name changed. Read the draft again before publishing.')
      const errors = draft.issues.filter((issue) => issue.severity === 'error')
      if (errors.length) throw new Error('The draft has ' + errors.length + ' validation error(s); fix them before publishing.')
      if (!draft.canPublish) throw new Error(draft.publishReason || 'Publishing is unavailable for this session.')

      const receipt = await publishSkill()
      await waitForVisibleUpdate()
      return { name: expectedName, jobId: receipt.jobId, status: receipt.status }
    },
  })
}
