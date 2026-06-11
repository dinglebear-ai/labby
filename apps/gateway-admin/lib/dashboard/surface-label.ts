import type { CallSurface } from '@/lib/types/metrics'

const SURFACE_LABEL: Partial<Record<CallSurface, string>> = {
  mcp: 'MCP',
  api: 'API',
  cli: 'CLI',
  web: 'Web',
  core_runtime: 'Core runtime',
  acp: 'ACP',
  dispatch: 'Dispatch',
  node: 'Node',
}

export function surfaceLabel(surface: CallSurface): string {
  return SURFACE_LABEL[surface] ?? surface.replaceAll('_', ' ')
}
