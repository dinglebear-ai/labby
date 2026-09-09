import { z } from 'zod'
import { getBrowserSessionEpoch } from '../auth/session-store'
import { normalizeGatewayApiBase } from './gateway-config'
import { gatewayRequestInit } from './gateway-request'
import { depotMembershipSource, type DepotMembershipSource } from './depot-membership-client'
import { SkillLibraryApiError } from './skill-library-client'

/** A mutation is never automatically replayed after authentication or transport failure. */
export function createDepotImportAttempt(source: DepotMembershipSource, libraryVersion: number, epoch: number) {
  const exact = depotMembershipSource(source)
  if (!exact) throw new Error('An exact Depot connection, Artifact, and revision are required.')
  const params = {
    source: { kind: 'depot', ...exact },
    expected_library_version: z.number().int().nonnegative().safe().parse(libraryVersion),
    idempotency_key: `depot-import-${crypto.randomUUID()}`,
  }
  let started = false
  const assertContext = () => {
    if (getBrowserSessionEpoch() !== epoch) throw new Error('Your session or project changed. Review the current library before importing again.')
  }
  return async () => {
    assertContext()
    if (started) throw new Error('This import was already submitted. Check your library before importing again.')
    started = true
    const response = await fetch(`${normalizeGatewayApiBase()}/artifacts`, gatewayRequestInit('artifacts.import', params))
    assertContext()
    if (!response.ok) {
      const parsed = z.object({ message: z.string().optional(), kind: z.string().optional(), code: z.string().optional(), param: z.string().optional() })
        .safeParse(await response.json().catch(() => ({})))
      const error = parsed.success ? parsed.data : {}
      throw new SkillLibraryApiError(error.message || 'Import was not confirmed. Check your library before importing again.', response.status, error.kind || error.code, error.param)
    }
    const result: unknown = await response.json()
    assertContext()
    return z.object({
      outcome: z.enum(['committed', 'replayed']),
      artifact_id: z.literal(exact.artifact_id),
      committed_library_version: z.literal(params.expected_library_version + 1),
      published_library_version: z.number().int().nonnegative().safe(),
    }).parse(result)
  }
}
