export const GATEWAY_COLUMN_ORDER_KEY = 'labby-gateway-col-order-v2'
export const GATEWAY_COLUMNS = ['clients', 'endpoint', 'exposed', 'uptime'] as const
export type GatewayColumn = typeof GATEWAY_COLUMNS[number]
export const GATEWAY_COLUMN_WIDTH: Record<GatewayColumn, string> = { clients: '80px', endpoint: 'minmax(140px,300px)', exposed: '170px', uptime: '130px' }
export function normalizeGatewayColumns(value: unknown): GatewayColumn[] {
  const known = Array.isArray(value) ? value.filter((item): item is GatewayColumn => typeof item === 'string' && (GATEWAY_COLUMNS as readonly string[]).includes(item)) : []
  return [...new Set([...known, ...GATEWAY_COLUMNS])]
}
export function visibleGatewayColumns(order: GatewayColumn[], width: number): GatewayColumn[] {
  return order.filter(column => !(column === 'uptime' && width < 1400) && !(column === 'clients' && width < 1180))
}
export function moveGatewayColumn(order: GatewayColumn[], source: GatewayColumn, target: GatewayColumn): GatewayColumn[] {
  if (source === target || !order.includes(source) || !order.includes(target)) return order
  const next = order.filter(column => column !== source)
  next.splice(next.indexOf(target), 0, source)
  return next
}
