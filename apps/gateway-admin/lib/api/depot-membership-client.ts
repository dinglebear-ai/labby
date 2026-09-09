import { z } from 'zod'
import { controlPlaneAction } from './artifact-control-client'
import { getBrowserSessionEpoch } from '../auth/session-store'

const sourceSchema = z.object({
  connection_id: z.string().min(1).max(128).regex(/^[A-Za-z0-9_-]+$/),
  artifact_id: z.string().min(1).max(160).regex(/^[a-z0-9][a-z0-9_-]*$/),
  revision_id: z.string().min(1).max(256),
}).strict()
const resultSchema = z.object({
  library_version: z.number().int().nonnegative().safe(),
  items: z.array(sourceSchema.extend({
    status: z.enum(['exact_revision_present', 'different_revision_present', 'absent']),
  }).strict()).max(100),
}).strict()

export type DepotMembershipSource = z.infer<typeof sourceSchema>
export type DepotMembershipResult = z.infer<typeof resultSchema>

export function depotMembershipSource(value: unknown): DepotMembershipSource | undefined {
  const parsed = sourceSchema.safeParse(value)
  return parsed.success ? parsed.data : undefined
}

export const depotMembershipKey = (source: DepotMembershipSource) => JSON.stringify([source.connection_id, source.artifact_id, source.revision_id])

/** Current latest revision only; every answer belongs to the browser authority epoch. */
export async function getDepotMembership(items: DepotMembershipSource[], signal?: AbortSignal): Promise<DepotMembershipResult> {
  const sources = z.array(sourceSchema).min(1).max(100).parse(items)
  const epoch = getBrowserSessionEpoch()
  const result = resultSchema.parse(await controlPlaneAction<unknown>('artifacts', 'artifacts.depot_membership', { items: sources }, signal))
  if (signal?.aborted || epoch !== getBrowserSessionEpoch()) {
    throw new Error('Library membership context changed; refresh the lookup.')
  }
  if (result.items.length !== sources.length || result.items.some((item, index) => {
    const source = sources[index]
    return item.connection_id !== source.connection_id || item.artifact_id !== source.artifact_id || item.revision_id !== source.revision_id
  })) {
    throw new Error('Library membership response did not match the requested sources.')
  }
  return result
}
