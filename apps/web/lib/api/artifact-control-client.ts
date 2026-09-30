import { z } from 'zod'

import { normalizeGatewayApiBase } from './gateway-config'
import { gatewayHeaders } from './gateway-request'
import { getBrowserSessionEpoch, getBrowserSessionState, getSessionCsrfToken } from '../auth/session-store'
import { performServiceAction, refreshBrowserSession, type ServiceActionError } from './service-action-client'

export type ArtifactControlError = ServiceActionError

const boundedString = (max: number) => z.string().min(1).max(max)
const artifactDescriptorSchema = z.object({
  id: boundedString(2048),
  kind: boundedString(128),
  namespace: boundedString(512),
  name: boundedString(512),
}).passthrough()
const artifactProjectionSchema = z.object({
  descriptor: artifactDescriptorSchema,
  currentRevision: z.object({
    id: boundedString(512),
    contentDigest: boundedString(512),
    components: z.array(z.object({}).passthrough()).max(2_000),
  }).passthrough(),
  revisionCount: z.number().int().min(1),
  stateVersion: boundedString(512),
  license: z.object({}).passthrough(),
  lineage: z.object({ following: z.boolean().optional() }).passthrough(),
  provenance: z.object({}).passthrough(),
  publication: z.object({
    state: z.enum(['draft', 'listed', 'published', 'withdrawn']),
    visibility: z.enum(['private', 'unlisted', 'public']),
    distribution: z.enum(['metadata', 'bytes']),
  }).passthrough(),
}).passthrough()
const artifactReceiptSchema = z.object({ artifact: artifactProjectionSchema }).passthrough()
const artifactSummarySchema = z.object({
  id: boundedString(2048),
  kind: boundedString(128),
  namespace: boundedString(512),
  name: boundedString(512),
  currentRevisionId: boundedString(512),
  contentDigest: boundedString(512),
}).passthrough()
const artifactListReceiptSchema = z.object({
  artifacts: z.array(artifactSummarySchema).max(200),
  total: z.number().int().nonnegative(),
  nextCursor: boundedString(2048).optional(),
}).passthrough()

const skillBundleMemberSchema = z.object({
  namespace: boundedString(512),
  name: boundedString(512),
}).passthrough()
const skillBundleVersionSchema = z.object({
  number: z.number().int().min(1),
  publishedAt: boundedString(128),
  skills: z.number().int().nonnegative(),
}).passthrough()
const skillBundleDriftSchema = z.object({
  clean: z.boolean(),
  added: z.array(skillBundleMemberSchema),
  removed: z.array(skillBundleMemberSchema),
  changed: z.array(skillBundleMemberSchema),
  missing: z.array(skillBundleMemberSchema),
}).passthrough()
const skillBundleSummarySchema = z.object({
  slug: boundedString(512),
  description: z.string().max(4_096),
  visibility: z.enum(['public', 'bearer', 'oauth']),
  members: z.number().int().nonnegative(),
  versions: z.number().int().nonnegative(),
  latestVersion: z.number().int().min(1).nullish(),
  drift: skillBundleDriftSchema,
}).passthrough()
const skillBundleSchema = skillBundleSummarySchema.extend({
  draft: z.array(skillBundleMemberSchema),
  publishedVersions: z.array(skillBundleVersionSchema),
})
const skillBundleListReceiptSchema = z.object({
  bundles: z.array(skillBundleSummarySchema),
}).passthrough()
const skillBundlePublishReceiptSchema = z.object({
  bundle: skillBundleSummarySchema,
  version: skillBundleVersionSchema,
  mounts: z.array(boundedString(2_048)).length(2),
}).passthrough().superRefine((receipt, context) => {
  const expected = [
    `/b/${receipt.bundle.slug}/mcp`,
    `/b/${receipt.bundle.slug}/v${receipt.version.number}/mcp`,
  ]
  if (receipt.mounts[0] !== expected[0] || receipt.mounts[1] !== expected[1]) {
    context.addIssue({ code: 'custom', path: ['mounts'], message: 'mounts do not match the published bundle version' })
  }
})

export type RemoteArtifactProjection = z.infer<typeof artifactProjectionSchema>
export type RemoteArtifactReceipt = z.infer<typeof artifactReceiptSchema>
export type RemoteArtifactListReceipt = z.infer<typeof artifactListReceiptSchema>
export type SkillBundle = z.infer<typeof skillBundleSchema>
export type SkillBundleSummary = z.infer<typeof skillBundleSummarySchema>
export type SkillBundlePublishReceipt = z.infer<typeof skillBundlePublishReceiptSchema>

type ArtifactControlRequestOptions = { connectionId?: string; signal?: AbortSignal }
type PublicationPatch = {
  state?: 'draft' | 'listed' | 'published' | 'withdrawn'
  visibility?: 'private' | 'unlisted' | 'public'
  distribution?: 'metadata' | 'bytes'
}
type LicensePatch = {
  declared?: string | null
  detected?: unknown[]
  notices?: unknown[]
  redistribution?: 'metadata_only' | 'cache_for_index' | 'redistributable' | 'forkable' | 'restricted' | 'unknown'
  reviewState?: 'unreviewed' | 'reviewed' | 'disputed'
  takedownState?: 'none' | 'requested' | 'restricted' | 'removed'
  evidenceAt?: string
  metadata?: Record<string, unknown>
}

function createError(message: string, status: number, code?: string, param?: string): ArtifactControlError {
  return Object.assign(new Error(message), { name: 'ArtifactControlError', status, code, param })
}

export function controlPlaneAction<T>(service: 'artifacts' | 'sources' | 'jobs' | 'uploads' | 'bundles', action: string, params: object = {}, signal?: AbortSignal) {
  return performServiceAction<T, ArtifactControlError>({
    serviceLabel: 'Artifact control plane',
    url: `${normalizeGatewayApiBase()}/${service}`,
    action,
    params,
    signal,
    createError,
  })
}

function parseReceipt<T>(schema: z.ZodType<T, z.ZodTypeDef, unknown>, value: unknown, label: string): T {
  const parsed = schema.safeParse(value)
  if (parsed.success) return parsed.data
  const issue = parsed.error.issues[0]
  const path = issue?.path.length ? ` at ${issue.path.join('.')}` : ''
  throw createError(`Artifact control plane returned an incompatible ${label}${path}: ${issue?.message ?? 'invalid response'}`, 502, 'incompatible_response')
}

function withConnection(params: object, options?: ArtifactControlRequestOptions) {
  return { ...params, ...(options?.connectionId ? { connection_id: options.connectionId } : {}) }
}

async function artifactReceipt(action: string, params: object, options?: ArtifactControlRequestOptions) {
  const response = await controlPlaneAction<unknown>('artifacts', action, withConnection(params, options), options?.signal)
  return parseReceipt(artifactReceiptSchema, response, 'Artifact receipt')
}

export async function listRemoteArtifacts(
  params: { cursor?: string; kind?: string; limit?: number; query?: string } = {},
  options?: ArtifactControlRequestOptions,
): Promise<RemoteArtifactListReceipt> {
  const response = await controlPlaneAction<unknown>('artifacts', 'artifacts.list_remote', withConnection(params, options), options?.signal)
  return parseReceipt(artifactListReceiptSchema, response, 'Artifact list receipt')
}

export function getRemoteArtifact(id: string, options?: ArtifactControlRequestOptions): Promise<RemoteArtifactReceipt> {
  return artifactReceipt('artifacts.get_remote', { id }, options)
}

export function followRemoteArtifact(
  artifact: RemoteArtifactProjection,
  params: { following: boolean; upstreamArtifactId?: string; upstreamRevisionId?: string },
  options?: ArtifactControlRequestOptions,
): Promise<RemoteArtifactReceipt> {
  const current = parseReceipt(artifactProjectionSchema, artifact, 'Artifact lifecycle state')
  return artifactReceipt('artifacts.follow', {
    id: current.descriptor.id,
    expected_version: current.stateVersion,
    following: params.following,
    ...(params.upstreamArtifactId ? { upstream_artifact_id: params.upstreamArtifactId } : {}),
    ...(params.upstreamRevisionId ? { upstream_revision_id: params.upstreamRevisionId } : {}),
  }, options)
}

export function setRemoteArtifactPublication(
  artifact: RemoteArtifactProjection,
  patch: PublicationPatch,
  options?: ArtifactControlRequestOptions,
): Promise<RemoteArtifactReceipt> {
  const current = parseReceipt(artifactProjectionSchema, artifact, 'Artifact lifecycle state')
  return artifactReceipt('artifacts.set_publication', {
    id: current.descriptor.id,
    expected_version: current.stateVersion,
    ...patch,
  }, options)
}

export function setRemoteArtifactLicense(
  artifact: RemoteArtifactProjection,
  patch: LicensePatch,
  options?: ArtifactControlRequestOptions,
): Promise<RemoteArtifactReceipt> {
  const current = parseReceipt(artifactProjectionSchema, artifact, 'Artifact lifecycle state')
  return artifactReceipt('artifacts.set_license', {
    id: current.descriptor.id,
    expected_version: current.stateVersion,
    ...(patch.declared !== undefined ? { declared: patch.declared } : {}),
    ...(patch.detected !== undefined ? { detected: patch.detected } : {}),
    ...(patch.notices !== undefined ? { notices: patch.notices } : {}),
    ...(patch.redistribution !== undefined ? { redistribution: patch.redistribution } : {}),
    ...(patch.reviewState !== undefined ? { review_state: patch.reviewState } : {}),
    ...(patch.takedownState !== undefined ? { takedown_state: patch.takedownState } : {}),
    ...(patch.evidenceAt !== undefined ? { evidence_at: patch.evidenceAt } : {}),
    ...(patch.metadata !== undefined ? { metadata: patch.metadata } : {}),
  }, options)
}

async function skillBundleReceipt(action: string, params: object, options?: ArtifactControlRequestOptions) {
  const response = await controlPlaneAction<unknown>('bundles', action, withConnection(params, options), options?.signal)
  return parseReceipt(skillBundleSchema, response, 'Skill bundle receipt')
}

export async function listSkillBundles(options?: ArtifactControlRequestOptions): Promise<SkillBundleSummary[]> {
  const response = await controlPlaneAction<unknown>('bundles', 'bundles.list', withConnection({}, options), options?.signal)
  return parseReceipt(skillBundleListReceiptSchema, response, 'Skill bundle list receipt').bundles
}

export function getSkillBundle(slug: string, options?: ArtifactControlRequestOptions): Promise<SkillBundle> {
  return skillBundleReceipt('bundles.get', { slug }, options)
}

export function createSkillBundle(
  params: { slug: string; description?: string; visibility?: 'public' | 'bearer' | 'oauth' },
  options?: ArtifactControlRequestOptions,
): Promise<SkillBundle> {
  return skillBundleReceipt('bundles.create', params, options)
}

export function addSkillToBundle(slug: string, namespace: string, name: string, options?: ArtifactControlRequestOptions): Promise<SkillBundle> {
  return skillBundleReceipt('bundles.add', { slug, namespace, name }, options)
}

export function removeSkillFromBundle(slug: string, namespace: string, name: string, options?: ArtifactControlRequestOptions): Promise<SkillBundle> {
  return skillBundleReceipt('bundles.remove', { slug, namespace, name }, options)
}

export function setSkillBundleVisibility(slug: string, visibility: 'public' | 'bearer' | 'oauth', options?: ArtifactControlRequestOptions): Promise<SkillBundle> {
  return skillBundleReceipt('bundles.set_visibility', { slug, visibility }, options)
}

export async function publishSkillBundle(slug: string, options?: ArtifactControlRequestOptions): Promise<SkillBundlePublishReceipt> {
  const response = await controlPlaneAction<unknown>('bundles', 'bundles.publish', withConnection({ slug }, options), options?.signal)
  return parseReceipt(skillBundlePublishReceiptSchema, response, 'Skill bundle publish receipt')
}

export async function uploadArtifactBytes(uploadId: string, file: File, connectionId?: string) {
  const query = connectionId ? `?connection_id=${encodeURIComponent(connectionId)}` : ''
  const initialCsrfToken = getSessionCsrfToken()
  const initialContext = getBrowserSessionEpoch()
  const assertCurrentContext = () => {
    if (initialContext !== getBrowserSessionEpoch()) {
      throw new DOMException('Authority or project context changed', 'AbortError')
    }
  }

  const request = async () => {
    assertCurrentContext()
    const headers = new Headers(gatewayHeaders())
    headers.set('Content-Type', file.type || 'application/octet-stream')
    const response = await fetch(`${normalizeGatewayApiBase()}/uploads/${encodeURIComponent(uploadId)}${query}`, {
      method: 'PUT', headers, body: file, credentials: 'include', cache: 'no-store',
    })
    const body = response.ok
      ? await response.json()
      : await response.json().catch(() => ({ message: 'Upload failed' }))
    assertCurrentContext()
    if (response.ok) return body as Record<string, unknown>
    throw createError(body.message || 'Upload failed', response.status, body.kind || body.code, body.param)
  }

  try {
    return await request()
  } catch (error) {
    assertCurrentContext()
    const uploadError = error as ArtifactControlError
    const authFailure = [401, 403, 422].includes(uploadError.status) && (
      uploadError.code === 'auth_failed' ||
      (Boolean(initialCsrfToken) && uploadError.code === 'validation_failed' && uploadError.message.toLowerCase().includes('csrf'))
    )
    if (!authFailure) throw error
    const current = getBrowserSessionState()
    if (current.status !== 'authenticated' || current.csrfToken === initialCsrfToken) {
      const refreshed = await refreshBrowserSession()
      if (refreshed.status !== 'authenticated') throw error
    }
    return request()
  }
}
