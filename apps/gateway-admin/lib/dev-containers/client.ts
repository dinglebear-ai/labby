import { performServiceAction, type ServiceActionError } from '@/lib/api/service-action-client'
import { assertGatewayAuthorityCurrent, captureGatewayAuthority } from '@/lib/api/gateway-request'
import { getSessionAuthority } from '@/lib/auth/session-store'

export type DevContainer = { instance_id: string; owner_kind: 'installation' | 'team' | 'project' | 'personal'; owner_id: string; desired_state: 'running' | 'stopped' | 'deleted'; observed_state: string }

export class DevContainerError extends Error implements ServiceActionError {
  constructor(message: string, public status: number, public code?: string) { super(message); this.name = 'DevContainerError' }
}

async function action<T>(name: string, params: object, signal?: AbortSignal): Promise<T> {
  const request = captureGatewayAuthority(signal)
  try {
    const value = await performServiceAction<T, DevContainerError>({ action: name, params, signal: request.signal, serviceLabel: 'Dev Containers', url: '/v1/dev-containers', createError: (message, status, code) => new DevContainerError(message, status, code) })
    assertGatewayAuthorityCurrent(request.generation)
    return value
  } finally { request.finish() }
}

export async function listDevContainers(signal?: AbortSignal) {
  const instances: DevContainer[] = []
  const seen = new Set<string>()
  let cursor: string | undefined
  for (let count = 0; count < 100; count++) {
    const page = await action<{ instances: DevContainer[]; next_cursor?: string | null }>('dev_containers.list', cursor ? { cursor } : {}, signal)
    instances.push(...page.instances)
    if (!page.next_cursor || page.instances.length === 0) return instances
    if (seen.has(page.next_cursor)) throw new DevContainerError('Container inventory returned a repeated page cursor', 502)
    seen.add(page.next_cursor)
    cursor = page.next_cursor
  }
  throw new DevContainerError('Container inventory exceeds the bounded listing limit', 502)
}
export async function listApprovedDevContainerTemplates(signal?: AbortSignal) {
  const owner = getSessionAuthority()?.activeOwner
  if (!owner) throw new DOMException('Authority is unavailable', 'InvalidStateError')
  const templates: string[] = []
  const seen = new Set<string>()
  let cursor: string | undefined
  for (let count = 0; count < 100; count++) {
    const page = await action<{ templates: string[]; next_cursor?: string | null }>('dev_containers.approved_templates.list', { owner_kind: owner.kind, owner_id: owner.id, ...(cursor ? { cursor } : {}) }, signal)
    templates.push(...page.templates)
    if (!page.next_cursor || page.templates.length === 0) return templates
    if (seen.has(page.next_cursor)) throw new DevContainerError('Approved templates returned a repeated page cursor', 502)
    seen.add(page.next_cursor)
    cursor = page.next_cursor
  }
  throw new DevContainerError('Approved template inventory exceeds the bounded listing limit', 502)
}
export async function createDevContainer(instanceId: string, templateId: string) {
  const owner = getSessionAuthority()?.activeOwner
  if (!owner) throw new DOMException('Authority is unavailable', 'InvalidStateError')
  return action('dev_containers.create', { instance_id: instanceId, template_id: templateId, owner_kind: owner.kind, owner_id: owner.id })
}
export function operateDevContainer(instanceId: string, operation: 'start' | 'stop' | 'destroy' | 'reconcile') { return action(`dev_containers.${operation}`, { instance_id: instanceId }) }
