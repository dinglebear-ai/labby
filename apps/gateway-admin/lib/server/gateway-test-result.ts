import type { TestGatewayResult } from '../types/gateway'
import type { BackendGatewayRuntimeView } from './gateway-adapter'

export function testResultFromProbe(
  runtime: BackendGatewayRuntimeView,
  probe: { connected: boolean; healthy: boolean; last_error?: string },
  detail?: string,
  // Wall-clock time the `gateway.test` action itself took, measured by the
  // caller. The backend runtime view carries no timing field, so this is the
  // only latency figure available — attaching it here (rather than leaving
  // `latency_ms` unset) is what fixed the toast that used to read
  // "Connection successful: undefinedms latency" (bead lab-bsblg).
  elapsedMs?: number,
): TestGatewayResult {
  if (!probe.connected) {
    return {
      success: false,
      severity: 'failure',
      message: 'Connection test failed',
      latency_ms: elapsedMs,
      discovered_tools: runtime.tool_count,
      discovered_resources: runtime.resource_count,
      discovered_prompts: runtime.prompt_count,
      error: detail ?? 'Gateway probe completed, but no usable MCP capabilities were discovered.',
    }
  }

  if (!probe.healthy) {
    return {
      success: true,
      severity: 'warning',
      message: 'Connection test passed with warnings',
      latency_ms: elapsedMs,
      discovered_tools: runtime.tool_count,
      discovered_resources: runtime.resource_count,
      discovered_prompts: runtime.prompt_count,
      detail:
        detail ??
        'The gateway connected successfully, but one or more optional MCP capabilities could not be discovered.',
    }
  }

  return {
    success: true,
    severity: 'success',
    message: 'Connection test passed',
    latency_ms: elapsedMs,
    discovered_tools: runtime.tool_count,
    discovered_resources: runtime.resource_count,
    discovered_prompts: runtime.prompt_count,
  }
}
