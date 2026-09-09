'use client'

import { useEffect, useState, useSyncExternalStore } from 'react'
import type { FederatedArtifact } from '@/lib/api/depot-client'
import { depotMembershipKey, depotMembershipSource, getDepotMembership, type DepotMembershipSource } from '@/lib/api/depot-membership-client'
import { getBrowserSessionEpoch, subscribeToBrowserSession } from '@/lib/auth/session-store'

export function membershipSourceForArtifact(artifact: FederatedArtifact) {
  return depotMembershipSource({
    connection_id: artifact.providerId,
    artifact_id: artifact.artifactId,
    revision_id: artifact.currentRevisionId ?? artifact.currentRevision?.id,
  })
}

export function discoverMembershipSources(artifacts: FederatedArtifact[]) {
  const unique = new Map<string, DepotMembershipSource>()
  for (const artifact of artifacts) {
    const source = membershipSourceForArtifact(artifact)
    if (source) unique.set(depotMembershipKey(source), source)
  }
  // Layout, local sorting, and opening an already-visible inspector do not change the query.
  return [...unique.entries()].sort(([left], [right]) => left < right ? -1 : left > right ? 1 : 0).map(([, source]) => source)
}

/** Visible-window-only checks; no library scan and no cross-session membership cache. */
export function useDiscoverMembership(artifacts: FederatedArtifact[], refreshVersion: number) {
  const epoch = useSyncExternalStore(subscribeToBrowserSession, getBrowserSessionEpoch, () => -1)
  const sources = discoverMembershipSources(artifacts)
  const requestKey = JSON.stringify([epoch, refreshVersion, sources])
  const [answer, setAnswer] = useState<{ key: string; present: Set<string> }>()
  useEffect(() => {
    const [, , requested] = JSON.parse(requestKey) as [number, number, DepotMembershipSource[]]
    if (!requested.length) return
    const controller = new AbortController()
    void (async () => {
      const present = new Set<string>()
      let version: number | undefined
      for (let offset = 0; offset < requested.length; offset += 100) {
        const result = await getDepotMembership(requested.slice(offset, offset + 100), controller.signal)
        if (version !== undefined && version !== result.library_version) throw new Error('Library changed during membership lookup.')
        version = result.library_version
        for (const item of result.items) if (item.status === 'exact_revision_present') present.add(depotMembershipKey(item))
      }
      if (!controller.signal.aborted) setAnswer({ key: requestKey, present })
    })().catch(() => {
      // Unknown is not absence: leave cards importable and never show an unproven badge.
      if (!controller.signal.aborted) setAnswer(undefined)
    })
    return () => controller.abort()
  }, [requestKey])
  return (artifact: FederatedArtifact) => {
    const source = membershipSourceForArtifact(artifact)
    return Boolean(source && answer?.key === requestKey && answer.present.has(depotMembershipKey(source)))
  }
}
