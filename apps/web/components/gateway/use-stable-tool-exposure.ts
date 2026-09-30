import { useMemo } from 'react'

import type { DiscoveredTool } from '@/lib/types/gateway'
import { createExposureDraftFromTools } from '@/lib/api/tool-exposure-draft'

interface StableToolExposureSnapshot {
  signature: string
  allToolNames: string[]
  currentExposedToolNames: string[]
}

/**
 * Keep derived tool-name arrays referentially stable when SWR returns a fresh
 * discovery array whose exposure-relevant contents have not changed.
 */
export function useStableToolExposure(
  tools: DiscoveredTool[],
): StableToolExposureSnapshot {
  const signature = JSON.stringify(
    tools.map(({ name, exposed }) => [name, exposed] as const),
  )

  return useMemo(
    () => ({
      signature,
      allToolNames: tools.map((tool) => tool.name),
      currentExposedToolNames: createExposureDraftFromTools(tools),
    }),
    // `signature` represents exactly the fields consumed by these derivations.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [signature],
  )
}
