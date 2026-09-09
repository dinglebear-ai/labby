import type { Gateway } from '@/lib/types/gateway'
export type GatewayBatchResult = { ok: true } | { ok: false; error: string }
export type GatewayBatchAction = 'enable' | 'disable' | 'reload'
export type GatewayBatchTarget = { id: string; name: string }
export type GatewayBatchReport = GatewayBatchTarget & { outcome: 'completed' | 'skipped' | 'failed'; detail?: string }
export type GatewayBatchCallbacks = {
  onBatchSetEnabled?: (gateway: Gateway, enabled: boolean) => Promise<GatewayBatchResult>
  onBatchReload?: (gateway: Gateway) => Promise<GatewayBatchResult>
}

export async function runGatewayBatch(action: GatewayBatchAction, targets: GatewayBatchTarget[], current: () => Gateway[], callbacks: GatewayBatchCallbacks, isCurrent: () => boolean): Promise<GatewayBatchReport[]> {
  const reports: GatewayBatchReport[] = []
  for (const target of targets) {
    if (!isCurrent()) { reports.push({ ...target, outcome: 'failed', detail: 'Session changed; operation not sent.' }); continue }
    const gateway = current().find(item => item.id === target.id)
    if (!gateway || gateway.name !== target.name) { reports.push({ ...target, outcome: 'skipped', detail: 'Server is no longer available under the confirmed identity.' }); continue }
    if (action !== 'reload' && (gateway.enabled ?? true) === (action === 'enable')) { reports.push({ ...target, outcome: 'skipped', detail: 'Server already has the requested enabled state.' }); continue }
    if (action === 'reload' && gateway.transport === 'in_process') { reports.push({ ...target, outcome: 'skipped', detail: 'This server does not support reload.' }); continue }
    try {
      const result = action === 'reload' ? await callbacks.onBatchReload?.(gateway) : await callbacks.onBatchSetEnabled?.(gateway, action === 'enable')
      reports.push({ ...target, outcome: result?.ok ? 'completed' : 'failed', detail: result && !result.ok ? result.error : result ? undefined : 'Operation is unavailable.' })
    } catch (error) { reports.push({ ...target, outcome: 'failed', detail: error instanceof Error ? error.message : 'Operation failed.' }) }
  }
  return reports
}
