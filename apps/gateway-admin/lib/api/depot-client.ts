import { z } from 'zod'

import { getBrowserSessionEpoch, getBrowserSessionState, getSessionCsrfToken } from '../auth/session-store'
import { gatewayRequestInit } from './gateway-request'
import { refreshBrowserSession } from './service-action-client'

const COMPATIBILITY_SCHEMA = 'labby.depot-compatibility/v1'
const FEDERATED_SCHEMA = 'labby.depot-compatibility/v2'
const bounded = (max: number) => z.string().max(max)
const rawId = bounded(2048).refine(value => new TextEncoder().encode(value).length <= 2048, 'artifact ID exceeds 2048 UTF-8 bytes')
const cursorSchema = z.string().length(43).regex(/^[A-Za-z0-9_-]+$/)

const contractSchema = z.object({
  schemaVersion: z.literal(COMPATIBILITY_SCHEMA),
  contractVersion: z.literal(1).optional(),
}).passthrough()

const schemaScalar = z.union([z.string().max(4096), z.number().safe()])
const schemaItem = z.object({
  type: z.enum(['string', 'boolean', 'integer', 'number', 'object', 'array']),
  enum: z.array(schemaScalar).max(100).optional(),
  minimum: z.number().safe().optional(), maximum: z.number().safe().optional(),
  minLength: z.number().int().min(0).max(16_384).optional(), maxLength: z.number().int().min(0).max(16_384).optional(),
  pattern: bounded(512).optional(),
  minProperties: z.number().int().min(0).max(256).optional(), maxProperties: z.number().int().min(0).max(256).optional(),
}).strict()
const operationPropertySchema = schemaItem.extend({
  description: bounded(4096).optional(),
  default: z.union([schemaScalar, z.boolean(), z.array(schemaScalar).max(100), z.record(z.string(), schemaScalar)]).optional(),
  minItems: z.number().int().min(0).max(1000).optional(), maxItems: z.number().int().min(0).max(1000).optional(),
  uniqueItems: z.boolean().optional(), items: schemaItem.optional(),
}).strict().superRefine((property, context) => {
  if (property.minimum !== undefined && property.maximum !== undefined && property.minimum > property.maximum) context.addIssue({ code: 'custom', message: 'minimum exceeds maximum' })
  if (property.minLength !== undefined && property.maxLength !== undefined && property.minLength > property.maxLength) context.addIssue({ code: 'custom', message: 'minLength exceeds maxLength' })
  if (property.minItems !== undefined && property.maxItems !== undefined && property.minItems > property.maxItems) context.addIssue({ code: 'custom', message: 'minItems exceeds maxItems' })
  if (property.minProperties !== undefined && property.maxProperties !== undefined && property.minProperties > property.maxProperties) context.addIssue({ code: 'custom', message: 'minProperties exceeds maxProperties' })
  if (property.pattern !== undefined) try { new RegExp(property.pattern) } catch { context.addIssue({ code: 'custom', message: 'pattern is not a valid regular expression' }) }
  const hasNumeric = property.minimum !== undefined || property.maximum !== undefined
  const hasString = property.minLength !== undefined || property.maxLength !== undefined || property.pattern !== undefined
  const hasArray = property.minItems !== undefined || property.maxItems !== undefined || property.uniqueItems !== undefined || property.items !== undefined
  const hasObject = property.minProperties !== undefined || property.maxProperties !== undefined
  if (hasNumeric && property.type !== 'integer' && property.type !== 'number') context.addIssue({ code: 'custom', message: 'numeric constraints require a numeric type' })
  if (hasString && property.type !== 'string') context.addIssue({ code: 'custom', message: 'string constraints require a string type' })
  if (hasArray && property.type !== 'array') context.addIssue({ code: 'custom', message: 'array constraints require an array type' })
  if (hasObject && property.type !== 'object') context.addIssue({ code: 'custom', message: 'object constraints require an object type' })
})
const operationPropertiesSchema = z.record(bounded(128), operationPropertySchema).superRefine((properties, context) => {
  if (Object.keys(properties).length > 128) context.addIssue({ code: 'custom', message: 'schema contains more than 128 properties' })
})
const operationSchema = z.object({
  name: bounded(256),
  title: bounded(512),
  description: bounded(4096),
  inputSchema: z.object({
    type: z.literal('object'),
    properties: operationPropertiesSchema.optional(),
    required: z.array(bounded(128)).max(128).optional(),
    additionalProperties: z.boolean().optional(),
  }).strict().superRefine((schema, context) => {
    const names = new Set(Object.keys(schema.properties ?? {}))
    for (const required of schema.required ?? []) if (!names.has(required)) context.addIssue({ code: 'custom', path: ['required'], message: `required property ${required} is not declared` })
    if (schema.additionalProperties === true) context.addIssue({ code: 'custom', path: ['additionalProperties'], message: 'additional properties are not supported' })
  }),
  annotations: z.object({ readOnlyHint: z.boolean().optional(), destructiveHint: z.boolean().optional(), idempotentHint: z.boolean().optional(), openWorldHint: z.boolean().optional() }).passthrough().optional(),
  group: z.enum(['catalog', 'access', 'operations']).optional(),
}).strict()
const operationsSchema = z.object({ operations: z.array(operationSchema).max(1000) }).passthrough()
const genericResultSchema = contractSchema.extend({ result: z.unknown() })

export type DepotOperation = z.infer<typeof operationSchema>

const depotStatusSchema = z.object({
  configured: z.boolean(), enabled: z.boolean(), mutationAuthority: z.boolean().optional(),
  authority: z.enum(['unknown', 'read', 'write']).optional(),
  maxResponseBytes: z.number().int().nonnegative(),
})

export type DepotArtifact = {
  id?: string
  kind?: string
  namespace?: string
  name?: string
  title?: string
  description?: string
  currentRevisionId?: string
  contentDigest?: string
  revisionCount?: number
  descriptor?: {
    id?: string
    kind?: string
    namespace?: string
    name?: string
    title?: string
    description?: string
  }
  currentRevision?: {
    id?: string
    contentDigest?: string
    createdAt?: string
    components?: Array<{ id?: string; kind?: string; path?: string; mediaType?: string; size?: number }>
  }
  publication?: { state?: string; visibility?: string; distribution?: string }
  license?: { redistribution?: string; reviewState?: string; takedownState?: string }
  lineage?: { following?: boolean; upstreamArtifactId?: string }
}

const artifactSchema: z.ZodType<DepotArtifact> = z.object({
  id: z.string().optional(), kind: z.string().optional(), namespace: z.string().optional(),
  name: z.string().optional(), title: z.string().optional(), description: z.string().optional(),
  currentRevisionId: z.string().optional(), contentDigest: z.string().optional(),
  revisionCount: z.number().int().nonnegative().optional(),
  descriptor: z.object({ id: z.string().optional(), kind: z.string().optional(), namespace: z.string().optional(), name: z.string().optional(), title: z.string().optional(), description: z.string().optional() }).passthrough().optional(),
  currentRevision: z.object({ id: z.string().optional(), contentDigest: z.string().optional(), createdAt: z.string().optional(), components: z.array(z.object({ id: z.string().optional(), kind: z.string().optional(), path: z.string().optional(), mediaType: z.string().optional(), size: z.number().nonnegative().optional() }).passthrough()).optional() }).passthrough().optional(),
  publication: z.object({ state: z.string().optional(), visibility: z.string().optional(), distribution: z.string().optional() }).passthrough().optional(),
  license: z.object({ redistribution: z.string().optional(), reviewState: z.string().optional(), takedownState: z.string().optional() }).passthrough().optional(),
  lineage: z.object({ following: z.boolean().optional(), upstreamArtifactId: z.string().optional() }).passthrough().optional(),
}).passthrough().refine((artifact) => Boolean(artifact.id?.trim() || artifact.descriptor?.id?.trim()), { message: 'artifact identity is missing' })

const listSchema = contractSchema.extend({ result: z.object({ artifacts: z.array(artifactSchema), nextCursor: z.string().optional(), total: z.number().int().nonnegative().optional() }).passthrough() })
const detailSchema = contractSchema.extend({ result: z.object({ artifact: artifactSchema }).passthrough() })

export type DepotStatus = z.infer<typeof depotStatusSchema>

async function parse(response: Response): Promise<unknown> {
  let body: unknown
  try { body = await response.json() } catch { throw new Error(`Depot returned invalid JSON (${response.status})`) }
  if (!response.ok) {
    const error = body && typeof body === 'object' ? body as Record<string, unknown> : {}
    const summary = typeof error.error === 'string' ? error.error : typeof error.message === 'string' ? error.message : `Depot request failed (${response.status})`
    throw new Error(safeDepotError(summary, response.status))
  }
  return body
}

function safeDepotError(value: string, status: number): string {
  return /^[a-z][a-z0-9_]{0,127}$/.test(value) ? `Depot request failed (${status}, ${value})` : `Depot request failed (${status})`
}

function validate<T>(schema: z.ZodType<T>, value: unknown, label: string): T {
  const result = schema.safeParse(value)
  if (result.success) return result.data
  const issue = result.error.issues[0]
  const path = issue?.path.length ? ` at ${issue.path.join('.')}` : ''
  throw new Error(`Depot returned an incompatible ${label}${path}: ${issue?.message ?? 'invalid response'}`)
}

export async function depotStatus(signal?: AbortSignal): Promise<DepotStatus> {
  const response = await fetch('/v1/depot/status', { credentials: 'same-origin', signal })
  return validate(z.object({ depot: depotStatusSchema }).passthrough(), await parse(response), 'status response').depot
}

export async function depotOperations(signal?: AbortSignal): Promise<DepotOperation[]> {
  const response = await fetch('/v1/depot/operations', { credentials: 'same-origin', cache: 'no-store', signal })
  return validate(operationsSchema, await parse(response), 'operation catalog response').operations
}

export async function depotCall<T>(operation: string, params: Record<string, unknown>, signal?: AbortSignal, destructiveIntent?: { confirmed: true; idempotencyKey: string }): Promise<T> {
  const init = gatewayRequestInit(operation, params, undefined, signal)
  init.body = JSON.stringify({ operation, params, ...(destructiveIntent ? { destructiveIntent } : {}) })
  const value = await parse(await fetch('/v1/depot/operations', init))
  if (operation === 'depot.artifacts.list') return validate(listSchema, value, 'artifact list response') as T
  if (operation === 'depot.artifacts.get') return validate(detailSchema, value, 'artifact detail response') as T
  return validate(genericResultSchema, value, 'operation response') as T
}

const federatedArtifactSchema = z.object({
  providerId: bounded(64), artifactId: rawId, id: rawId.optional(), kind: bounded(128).optional(),
  namespace: bounded(512).optional(), name: bounded(512).optional(), title: bounded(4096).optional(),
  description: bounded(16384).optional(), currentRevisionId: bounded(512).optional(),
  contentDigest: bounded(512).optional(),
  license: z.object({ redistribution: bounded(128).optional(), reviewState: bounded(128).optional(), takedownState: bounded(128).optional() }).strict().optional(),
  publication: z.object({ state: bounded(128).optional(), visibility: bounded(128).optional(), distribution: bounded(128).optional() }).strict().optional(),
  revisionCount: z.number().safe().int().nonnegative().optional(),
  descriptor: z.object({ id: rawId.optional(), kind: bounded(128).optional(), namespace: bounded(512).optional(), name: bounded(512).optional(), title: bounded(4096).optional(), description: bounded(16384).optional() }).strict().optional(),
  currentRevision: z.object({ id: bounded(512).optional(), contentDigest: bounded(512).optional() }).strict().optional(),
}).strict()
const outcomeSchema = z.object({ providerId: bounded(64), state: z.enum(['pending', 'participating', 'exhausted', 'failed']) }).strict()
const failureSchema = z.object({ providerId: bounded(64), kind: bounded(128) }).strict()
const discoverySchema = z.object({
  schemaVersion: z.literal(FEDERATED_SCHEMA), scope: bounded(64), scopeEpoch: bounded(128),
  items: z.array(federatedArtifactSchema).max(200), providerOutcomes: z.array(outcomeSchema).max(16),
  failures: z.array(failureSchema).max(16), coverageComplete: z.boolean(),
  knownTotal: z.number().safe().int().nonnegative().nullable().optional(), totalIsExact: z.boolean(),
  state: z.enum(['complete', 'partial', 'deferred', 'empty', 'all_disabled', 'all_failed']),
  nextCursor: cursorSchema.nullable().optional(),
}).strict()
const detailArtifactSchema = federatedArtifactSchema.omit({ providerId: true, artifactId: true }).extend({ id: rawId }).strict()
const detailV2Schema = z.object({
  schemaVersion: z.literal(FEDERATED_SCHEMA), providerId: bounded(64), artifactId: rawId,
  artifact: detailArtifactSchema,
}).strict()
const providerSchema = z.object({
  id: bounded(64).refine(value => value !== 'all'), name: bounded(256), endpoint: bounded(2048),
  enabled: z.boolean(), authMode: z.enum(['anonymous', 'bearer']), builtin: z.boolean(), configVersion: bounded(128), credentialConfigured: z.boolean(), health: z.object({
    state: z.enum(['unknown', 'healthy', 'unauthorized', 'incompatible', 'unavailable']),
    observedAt: z.number().safe().int().nonnegative().nullable(), provenance: bounded(64).nullable(),
    retryNotBefore: z.number().safe().int().nonnegative().nullable(),
  }).strict(),
}).strict()
const providerOptionSchema = z.object({ id: bounded(64).refine(value => value !== 'all'), name: bounded(256), enabled: z.boolean(), health: providerSchema.shape.health }).strict()

export type FederatedArtifact = z.infer<typeof federatedArtifactSchema>
export type DiscoveryPage = z.infer<typeof discoverySchema>
export type DepotProvider = z.infer<typeof providerSchema>
export type DepotProviderOption = z.infer<typeof providerOptionSchema>
export type CredentialOperation = { action: 'retain' } | { action: 'replace'; value: string } | { action: 'clear' }
const mutationSchema = z.object({ operationId: bounded(128), version: bounded(128), committed: z.boolean() }).strict()
const probeSchema = z.object({ providerId: bounded(64), state: z.enum(['healthy', 'unauthorized', 'incompatible', 'unavailable']), observedAt: z.number().safe().int().nonnegative() }).strict()

export type ProviderDraft = { id: string; name: string; endpoint: string; enabled: boolean; authMode: 'anonymous' | 'bearer'; credential: CredentialOperation; expectedVersion: string; operationId: string; proof?: string }

export class DepotClientError extends Error {
  constructor(public readonly status: number, public readonly kind: string, message: string, public readonly recovery?: unknown, public readonly requestId?: string) { super(message) }
}

class DepotSessionChangedError extends Error {}

async function requestV2<T>(path: string, init: RequestInit, schema: z.ZodType<T>, label: string, sessionCsrf: 'retry-once' | 'off' = 'off'): Promise<T> {
  const baseHeaders = new Headers(init.headers)
  const initialCsrfToken = getSessionCsrfToken()
  const request = async () => {
    const epoch = getBrowserSessionEpoch()
    const headers = new Headers(baseHeaders)
    const csrfToken = getSessionCsrfToken()
    if (sessionCsrf === 'retry-once' && csrfToken) headers.set('x-csrf-token', csrfToken)
    const response = await fetch(path, { credentials: 'same-origin', cache: 'no-store', ...init, headers })
    const requestId = response.headers.get('x-request-id') ?? undefined
    let body: unknown
    try { body = await response.json() } catch { throw new DepotClientError(response.status, 'invalid_response', `Depot returned invalid JSON (${response.status})`, undefined, requestId) }
    if (epoch !== getBrowserSessionEpoch()) throw new DepotSessionChangedError('Session changed')
    if (!response.ok) {
      const error = z.object({ kind: bounded(128), message: bounded(4096), recovery: z.unknown().optional() }).passthrough().safeParse(body)
      throw new DepotClientError(response.status, error.success ? error.data.kind : 'request_failed', error.success ? error.data.message : `Depot request failed (${response.status})`, error.success ? error.data.recovery : undefined, requestId)
    }
    return validate(schema, body, label)
  }

  try {
    return await request()
  } catch (error) {
    const currentSession = getBrowserSessionState()
    if (sessionCsrf === 'retry-once' && error instanceof DepotSessionChangedError &&
      currentSession.status === 'authenticated' && currentSession.csrfToken !== initialCsrfToken) return request()
    const staleSession = sessionCsrf === 'retry-once' && error instanceof DepotClientError &&
      [401, 403, 422].includes(error.status) &&
      (error.kind === 'auth_failed' || error.message.toLowerCase().includes('csrf'))
    if (!staleSession) throw error
    if (currentSession.status === 'authenticated' && currentSession.csrfToken !== initialCsrfToken) return request()
    const session = await refreshBrowserSession()
    if (session.status !== 'authenticated') throw error
    return request()
  }
}

export async function listArtifacts(input: { provider?: string; query?: string; limit?: number; cursor?: string } = {}, signal?: AbortSignal): Promise<DiscoveryPage> {
  const query = input.query ?? ''
  if (query.length > 200 || (query.length > 0 && query.length < 3)) throw new Error('Query must be empty or contain 3 to 200 characters')
  const provider = input.provider ?? 'all'
  if (provider !== 'all' && !/^[a-z0-9][a-z0-9_-]{0,63}$/.test(provider)) throw new Error('Invalid provider')
  const page = await requestV2('/v1/depot/discover', { method: 'POST', signal, headers: { 'content-type': 'application/json' }, body: JSON.stringify({ provider: provider === 'all' ? null : provider, query, limit: input.limit ?? 50, cursor: input.cursor }) }, discoverySchema, 'discovery response', 'retry-once')
  if (page.scope !== provider) throw new Error('Depot returned the wrong discovery scope')
  if (provider !== 'all' && page.items.some(item => item.providerId !== provider)) throw new Error('Depot returned an artifact from the wrong provider')
  return page
}

export async function getArtifact(providerId: string, artifactId: string, signal?: AbortSignal) {
  const value = await requestV2('/v1/depot/artifacts/detail', { method: 'POST', signal, headers: { 'content-type': 'application/json' }, body: JSON.stringify({ providerId, artifactId }) }, detailV2Schema, 'artifact detail response', 'retry-once')
  if (value.providerId !== providerId || value.artifactId !== artifactId || value.artifact.id !== artifactId) throw new Error('Depot returned the wrong artifact identity')
  return value
}

export async function listProviders(signal?: AbortSignal): Promise<DepotProvider[]> {
  return requestV2('/v1/depot/providers', { signal }, z.array(providerSchema).max(16), 'provider list response')
}

export async function listProviderOptions(signal?: AbortSignal): Promise<DepotProviderOption[]> {
  const providers = await requestV2('/v1/depot/providers', { signal }, z.array(z.union([providerOptionSchema, providerSchema])).max(16), 'provider options response')
  return providers.map(({ id, name, enabled, health }) => ({ id, name, enabled, health }))
}

export async function upsertProvider(input: ProviderDraft, csrf: string, signal?: AbortSignal) {
  return requestV2('/v1/depot/providers', { method: 'POST', signal, headers: { 'content-type': 'application/json', 'x-csrf-token': csrf }, body: JSON.stringify(input) }, mutationSchema, 'provider mutation response')
}

export async function removeProvider(providerId: string, expectedVersion: string, operationId: string, proof: string, csrf: string, signal?: AbortSignal) {
  return requestV2(`/v1/depot/providers/${encodeURIComponent(providerId)}`, { method: 'DELETE', signal, headers: { 'content-type': 'application/json', 'x-csrf-token': csrf }, body: JSON.stringify({ expectedVersion, operationId, proof }) }, mutationSchema, 'provider removal response')
}

export async function probeProvider(input: Pick<ProviderDraft, 'id' | 'name' | 'endpoint' | 'enabled' | 'authMode' | 'credential'>, csrf: string, signal?: AbortSignal) {
  return requestV2('/v1/depot/providers/probe', { method: 'POST', signal, headers: { 'content-type': 'application/json', 'x-csrf-token': csrf }, body: JSON.stringify(input) }, probeSchema, 'provider probe response')
}

export async function providerOperation(operationId: string, signal?: AbortSignal) {
  return requestV2(`/v1/depot/provider-operations/${encodeURIComponent(operationId)}`, { signal }, mutationSchema, 'provider operation response')
}
