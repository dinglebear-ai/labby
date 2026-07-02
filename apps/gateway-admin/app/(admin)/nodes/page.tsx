import { NodesPage } from '@/components/nodes/nodes-page'
import { CapabilityGuard } from '@/components/capability-guard'

export default function NodesPageRoute() {
  return (
    <CapabilityGuard need="nodes" label="Nodes">
      <NodesPage />
    </CapabilityGuard>
  )
}

