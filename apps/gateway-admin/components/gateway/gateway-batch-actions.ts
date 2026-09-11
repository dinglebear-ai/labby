import { getErrorMessage } from '@/lib/utils'

type Result = { ok: true } | { ok: false; error: string }
export function gatewayBatchActions(deps: {
  enable: (id: string) => Promise<unknown>
  disable: (id: string) => Promise<unknown>
  reload: (id: string) => Promise<{ success: boolean; message: string }>
}) {
  return {
    async setEnabled(gateway: { id: string }, enabled: boolean): Promise<Result> {
      try {
        await (enabled ? deps.enable(gateway.id) : deps.disable(gateway.id))
        return { ok: true }
      } catch (error) {
        return { ok: false, error: getErrorMessage(error, `Failed to ${enabled ? 'enable' : 'disable'} server`) }
      }
    },
    async reload(gateway: { id: string }): Promise<Result> {
      try {
        const result = await deps.reload(gateway.id)
        return result.success ? { ok: true } : { ok: false, error: result.message || 'Failed to reload server' }
      } catch (error) {
        return { ok: false, error: getErrorMessage(error, 'Failed to reload server') }
      }
    },
  }
}
