import type { Gateway } from '@/lib/types/gateway'

/**
 * The label to show for a server. `display_name` is presentation only; keep
 * addressing the server by `gateway.name` in API calls, keys, and routes.
 */
export function gatewayLabel(gateway: Pick<Gateway, 'name' | 'display_name'>): string {
  return gateway.display_name?.trim() || gateway.name
}
