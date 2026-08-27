/**
 * Shared confirmation copy for destructive gateway actions.
 *
 * `gateway.remove` is `destructive: true` in the shared action catalog, and the
 * remove dialog is reachable from two places — the server row menu and the
 * server detail page. Both rendered the same sentence, hand-duplicated, which
 * is exactly the shape that drifts into two subtly different warnings for one
 * irreversible action.
 */

/** Title for the remove-server confirmation, on every surface that offers it. */
export const REMOVE_GATEWAY_TITLE = 'Remove server?'

/** Confirm-button label for the remove-server confirmation. */
export const REMOVE_GATEWAY_CONFIRM_LABEL = 'Remove server'

/**
 * Body copy naming the server being deleted.
 *
 * Callers pass the resolved name; there is deliberately no "this server"
 * fallback, because every entry point opens the dialog from a row or page that
 * already knows which server it is, and quietly dropping the name would weaken
 * the one thing the dialog exists to state.
 */
export function removeGatewayDescription(name: string): string {
  return (
    `This permanently deletes ${name} from the gateway configuration. ` +
    'Connected clients lose access immediately and the configuration cannot be recovered.'
  )
}
