export type GatewaySaveRollback = () => Promise<void>

export class GatewaySaveCompensationError extends Error {
  constructor(readonly rollbackError: unknown) {
    super('The protected route failed and the server change could not be rolled back')
    this.name = 'GatewaySaveCompensationError'
  }
}

/**
 * Apply the gateway write and its protected-route write as one UI transaction.
 * The backend write supplies compensation because the two resources currently
 * have separate API actions.
 */
export async function runGatewaySaveTransaction(
  saveGateway: () => Promise<GatewaySaveRollback | void>,
  applyProtectedRoute: () => Promise<void>,
): Promise<void> {
  const rollback = await saveGateway()
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
}
