import { z } from 'zod'
import { controlPlaneAction } from './artifact-control-client'
import { getDepotMembership, type DepotMembershipSource } from './depot-membership-client'
import { getBrowserSessionEpoch } from '../auth/session-store'
import { gatewayRequestInit } from './gateway-request'
import { normalizeGatewayApiBase } from './gateway-config'
import { SkillLibraryApiError } from './skill-library-client'

const version = z.number().int().nonnegative().safe()
// VersionedSkillLibrarySummary flattens SkillLibrarySummary on the Rust wire contract.
const summarySchema = z.object({
  library_version: version,
  artifact_id: z.string(),
  archived: z.boolean(),
  materialized: z.boolean(),
  latest_revision_id: z.string(),
  active_revision_id: z.string().nullable().optional(),
  published_library_version: version,
  allowed_actions: z.array(z.string()).max(100),
})
const receiptSchema = z.object({
  outcome: z.enum(['committed', 'replayed']),
  artifact_id: z.string(),
  active_revision_id: z.string(),
  committed_library_version: version,
  published_library_version: version,
})

export type DepotSkillActivationReady = {
  status: 'ready'
  source: DepotMembershipSource
  libraryVersion: number
  epoch: number
}
export type DepotSkillActivationPreparation = DepotSkillActivationReady | {
  status: 'blocked' | 'active'
  message: string
}

function assertContext(epoch: number, signal?: AbortSignal) {
  if (signal?.aborted || getBrowserSessionEpoch() !== epoch) {
    throw new Error('Gateway session changed. Reopen Send to Labby and review the current context.')
  }
}

/** This is Skill publication in the connected gateway, not an arbitrary Artifact deployment. */
export async function prepareDepotSkillActivation(
  kind: string,
  source?: DepotMembershipSource,
  signal?: AbortSignal,
): Promise<DepotSkillActivationPreparation> {
  if (kind !== 'skill') return { status: 'blocked', message: 'Send to Labby currently supports Skills only. This Artifact kind has no supported activation operation.' }
  if (!source) return { status: 'blocked', message: 'This source has not supplied a configured connection and exact Artifact revision. No activation is available.' }
  const epoch = getBrowserSessionEpoch()
  const membership = await getDepotMembership([source], signal)
  assertContext(epoch, signal)
  if (membership.items[0].status !== 'exact_revision_present') {
    return { status: 'blocked', message: membership.items[0].status === 'different_revision_present'
      ? 'Your library contains a different latest revision. Review the library before activating this exact revision.'
      : 'Add this exact Skill revision to your library first, then return to Send to Labby. Nothing is imported automatically.' }
  }
  const summary = summarySchema.parse(await controlPlaneAction<unknown>('artifacts', 'artifacts.get', { artifact_id: source.artifact_id }, signal))
  assertContext(epoch, signal)
  if (summary.library_version !== membership.library_version || summary.artifact_id !== source.artifact_id || summary.latest_revision_id !== source.revision_id) {
    throw new Error('The library changed during this check. Refresh and review the exact revision again.')
  }
  if (summary.archived || !summary.materialized) return { status: 'blocked', message: 'This Skill is archived or is not materialized in your library. Review it in Library first.' }
  if (summary.active_revision_id === source.revision_id) {
    return { status: 'active', message: summary.published_library_version >= summary.library_version
      ? 'This exact Skill revision is already active in the connected gateway.'
      : 'This revision is marked active, but the gateway has not published the current library generation. Review Library reconciliation before retrying.' }
  }
  if (!summary.allowed_actions.includes('artifacts.activate')) return { status: 'blocked', message: 'You can view this Skill, but the server has not granted you permission to activate it.' }
  return { status: 'ready', source: { ...source }, libraryVersion: summary.library_version, epoch }
}

export interface DepotSkillActivationAttempt {
  readonly hasBeenSent: boolean
  /** Retry the same operation after an uncertain outcome; never change its CAS or idempotency key. */
  run: () => Promise<void>
}

async function activateOnce(params: object): Promise<unknown> {
  // Mutations must never refresh authentication and replay under a different browser authority.
  const response = await fetch(`${normalizeGatewayApiBase()}/artifacts`, gatewayRequestInit('artifacts.activate', params))
  if (!response.ok) {
    const body = z.object({ message: z.string().optional(), kind: z.string().optional(), code: z.string().optional(), param: z.string().optional() })
      .safeParse(await response.json().catch(() => ({})))
    const error = body.success ? body.data : {}
    throw new SkillLibraryApiError(error.message || 'Skill activation failed. Review Library before retrying.', response.status, error.kind || error.code, error.param)
  }
  return response.json()
}

export function createDepotSkillActivationAttempt(
  prepared: DepotSkillActivationReady,
  idempotencyKey = `depot-activate-${crypto.randomUUID()}`,
): DepotSkillActivationAttempt {
  const source = { ...prepared.source }
  const epoch = prepared.epoch
  const libraryVersion = prepared.libraryVersion
  let started = false
  let inFlight: Promise<void> | undefined
  const execute = async () => {
    assertContext(epoch)
    if (!started) {
      // Confirmation never relies solely on the state displayed when the dialog opened.
      const current = await prepareDepotSkillActivation('skill', source)
      assertContext(epoch)
      if (current.status !== 'ready' || current.libraryVersion !== libraryVersion) {
        throw new Error('Activation state changed. Close and reopen this dialog to review it before activating.')
      }
      started = true
    }
    const receipt = receiptSchema.parse(await activateOnce({
      artifact_id: source.artifact_id,
      expected_revision_id: source.revision_id,
      expected_library_version: libraryVersion,
      idempotency_key: idempotencyKey,
    }))
    assertContext(epoch)
    if (receipt.artifact_id !== source.artifact_id || receipt.active_revision_id !== source.revision_id
      || receipt.committed_library_version !== libraryVersion + 1
      || receipt.published_library_version < receipt.committed_library_version) {
      throw new Error('Activation was not confirmed as published. Retry this same operation to reconcile its outcome, or review Library.')
    }
    // Replayed receipts preserve old facts; verify the current state before claiming it is active.
    const current = summarySchema.parse(await controlPlaneAction<unknown>('artifacts', 'artifacts.get', { artifact_id: source.artifact_id }))
    assertContext(epoch)
    if (current.artifact_id !== source.artifact_id || current.archived
      || current.active_revision_id !== source.revision_id
      || current.library_version < receipt.committed_library_version
      || current.published_library_version < current.library_version) {
      throw new Error('The current gateway no longer confirms this exact active revision. Review Library before continuing.')
    }
  }
  return {
    get hasBeenSent() { return started },
    run() {
      if (!inFlight) inFlight = execute().finally(() => { inFlight = undefined })
      return inFlight
    },
  }
}
