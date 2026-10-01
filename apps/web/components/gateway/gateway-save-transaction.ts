export type GatewaySaveRollback = (() => Promise<void>) | {
  rollback?: () => Promise<void>
  commit?: () => void
}

export class GatewaySaveCompensationError extends Error {
  constructor(readonly rollbackError: unknown) {
    super('The protected route failed and the server change could not be rolled back')
    this.name = 'GatewaySaveCompensationError'
  }
}

/**
 * Commit UI navigation after the backend-owned atomic save completes.
 * Optional compensation remains available to callers coordinating other writes.
 */
export async function runGatewaySaveTransaction(
  saveGateway: () => Promise<GatewaySaveRollback | void>,
  applyProtectedRoute: () => Promise<void> = async () => {},
): Promise<void> {
  const saved = await saveGateway()
  const rollback = typeof saved === 'function' ? saved : saved?.rollback
  try {
    await applyProtectedRoute()
  } catch (error) {
    if (rollback) {
      try {
        await rollback()
      } catch (rollbackError) {
        throw new GatewaySaveCompensationError(rollbackError)
      }
    }
    throw error
  }
  if (saved && typeof saved !== 'function') saved.commit?.()
}
