import { z } from 'zod'
import { gatewayApi } from './gateway-client'
import { normalizeGatewayApiBase } from './gateway-config'
import { gatewayRequestInit } from './gateway-request'
import { depotMembershipSource, type DepotMembershipSource } from './depot-membership-client'
import { getBrowserSessionEpoch, getBrowserSessionState } from '../auth/session-store'

export type DepotForkReady = { status: 'ready'; source: DepotMembershipSource; epoch: number }
export type DepotForkPreparation = DepotForkReady | { status: 'blocked'; message: string }
const destination = z.string().min(1).max(128).regex(/^[A-Za-z0-9]+(?:[._-][A-Za-z0-9]+)*$/)

function assertEpoch(epoch: number, signal?: AbortSignal) {
  if (signal?.aborted || epoch !== getBrowserSessionEpoch()) throw new Error('Gateway session changed. Reopen Fork and review the selected source.')
}

export async function prepareDepotFork(source?: DepotMembershipSource, signal?: AbortSignal): Promise<DepotForkPreparation> {
  const exact = depotMembershipSource(source)
  if (!exact) return { status: 'blocked', message: 'This source has not supplied a configured Depot connection and exact revision. Fork is unavailable.' }
  const epoch = getBrowserSessionEpoch()
  const actions = await gatewayApi.serviceActions('artifacts', signal)
  assertEpoch(epoch, signal)
  const session = getBrowserSessionState()
  if (session.status !== 'authenticated' || !session.isAdmin) return { status: 'blocked', message: 'Forking hosted Artifacts requires an administrator in the current gateway.' }
  const action = actions.find(item => item.name === 'artifacts.fork')
  if (!action || action.destructive) return { status: 'blocked', message: 'The current gateway has not exposed the supported Fork operation.' }
  return { status: 'ready', source: exact, epoch }
}

/** One attempt only: an uncertain result must be reconciled in the hosted catalog. */
export function createDepotForkAttempt(prepared: DepotForkReady, namespace: string, name: string) {
  const source = { ...prepared.source }
  const epoch = prepared.epoch
  const target = { namespace: destination.parse(namespace), name: destination.parse(name) }
  let started = false
  return async () => {
    if (started) throw new Error('This fork was already submitted. Check the hosted catalog before creating another fork.')
    started = true
    assertEpoch(epoch)
    const current = await prepareDepotFork(source)
    assertEpoch(epoch)
    if (current.status !== 'ready') throw new Error(current.message)
    const response = await fetch(`${normalizeGatewayApiBase()}/artifacts`, gatewayRequestInit('artifacts.fork', {
      connection_id: source.connection_id, source_artifact_id: source.artifact_id,
      revision_id: source.revision_id, ...target, following: false,
    }))
    assertEpoch(epoch)
    const body: unknown = await response.json()
    if (!response.ok) {
      const error = z.object({ message: z.string().optional(), kind: z.string().optional(), code: z.string().optional(), param: z.string().optional() }).safeParse(body)
      throw Object.assign(new Error(error.success ? error.data.message ?? 'Fork was not confirmed. Check the hosted catalog.' : 'Fork was not confirmed. Check the hosted catalog.'), {
        status: response.status, code: error.success ? error.data.kind ?? error.data.code : undefined,
        param: error.success ? error.data.param : undefined,
      })
    }
    const result = z.object({ artifact: z.object({
      descriptor: z.object({ id: z.string().min(1), namespace: z.literal(target.namespace), name: z.literal(target.name) }),
      lineage: z.object({ forkedFromArtifactId: z.literal(source.artifact_id), forkedFromRevisionId: z.literal(source.revision_id) }),
    }) }).parse(body)
    assertEpoch(epoch)
    return result.artifact.descriptor.id
  }
}
