import type { CatalogService } from '@/lib/types/command-catalog'

/**
 * Whether `action` on `service` needs an explicit confirmation before it is
 * dispatched.
 *
 * Derived from the shared action catalog's `destructive` flag, the same
 * metadata that drives MCP elicitation and CLI confirmation, so the UI never
 * hand-picks which actions warn. An action the loaded catalog does not list
 * (catalog not yet loaded, or a UI newer than the server) fails closed and
 * asks rather than skipping the check for an irreversible action.
 */
export function actionRequiresConfirmation(
  services: readonly CatalogService[],
  service: string,
  action: string,
): boolean {
  const entry = services
    .find((candidate) => candidate.name === service)
    ?.actions.find((candidate) => candidate.action === action)
  return entry?.destructive ?? true
}

/** Shared copy for the destructive `agents.delete` confirmation. */
export const DELETE_AGENT_TITLE = 'Delete Agent definition?'
export const DELETE_AGENT_CONFIRM_LABEL = 'Delete Agent'

export function deleteAgentDescription(agentId: string): string {
  return (
    `This permanently deletes ${agentId} from the Agent catalog. ` +
    'Deleted definitions are filtered out of every read and cannot be restored; ' +
    'existing durable Task records remain immutable.'
  )
}
