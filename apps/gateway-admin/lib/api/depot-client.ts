import { z } from 'zod'

import { gatewayRequestInit } from './gateway-request'

const COMPATIBILITY_SCHEMA = 'labby.depot-compatibility/v1'

const contractSchema = z.object({
  schemaVersion: z.literal(COMPATIBILITY_SCHEMA),
  contractVersion: z.literal(1).optional(),
}).passthrough()

const depotStatusSchema = z.object({
  configured: z.boolean(), enabled: z.boolean(), mutationAuthority: z.boolean(),
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
    throw new Error(typeof error.error === 'string' ? error.error : typeof error.message === 'string' ? error.message : `Depot request failed (${response.status})`)
  }
  return body
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

export async function depotCall<T>(operation: string, params: Record<string, unknown>, signal?: AbortSignal): Promise<T> {
  const init = gatewayRequestInit(operation, params, undefined, signal)
  init.body = JSON.stringify({ operation, params })
  const value = await parse(await fetch('/v1/depot/operations', init))
  if (operation === 'depot.artifacts.list') return validate(listSchema, value, 'artifact list response') as T
  if (operation === 'depot.artifacts.get') return validate(detailSchema, value, 'artifact detail response') as T
  throw new Error(`Unsupported Depot operation: ${operation}`)
}
