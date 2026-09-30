import Link from 'next/link'
import { GitFork } from 'lucide-react'
import { AURORA_MUTED_LABEL } from '@/components/aurora/tokens'
import type { FederatedArtifact } from '@/lib/api/depot-client'

/** Depot returns lineage ids only for Artifacts the caller may see; this renders them without further checks. */
export function DiscoverUpstream({ artifact }: { artifact: FederatedArtifact }) {
  const lineage = artifact.lineage
  if (!lineage || (!lineage.upstreamArtifactId && !lineage.forkedFromArtifactId)) return null
  const relationships = [
    { label: 'Upstream Artifact', id: lineage.upstreamArtifactId, revision: lineage.upstreamRevisionId },
    { label: 'Forked from', id: lineage.forkedFromArtifactId, revision: lineage.forkedFromRevisionId },
  ]
  return <section aria-label="Upstream" className="rounded-aurora-2 border border-aurora-border-default bg-aurora-panel-medium">
    <h3 className={`${AURORA_MUTED_LABEL} flex items-center gap-[var(--space-2)] border-b border-aurora-border-default p-[var(--space-4)]`}><GitFork aria-hidden="true" className="size-3.5" />Upstream</h3>
    <div className="space-y-[var(--space-4)] p-[var(--space-4)] text-xs">
      {relationships.map(({ label, id, revision }) => id ? <div key={label}>
        <p className={AURORA_MUTED_LABEL}>{label}</p>
        <Link className="mt-[var(--space-2)] inline-block break-all text-aurora-accent-primary underline underline-offset-4 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary" href={`/depot?${new URLSearchParams({ artifactProvider: artifact.providerId, artifact: id })}`}>{id}</Link>
        {revision ? <p className="mt-[var(--space-2)] break-all font-mono text-aurora-text-muted">Revision: {revision}</p> : null}
      </div> : null)}
      {lineage.upstreamArtifactId ? <p className="text-aurora-text-muted">{lineage.following ? 'Following upstream.' : 'Not following upstream.'} This does not indicate synchronization status.</p> : null}
      {lineage.lastObservedUpstreamRevisionId ? <p className="break-all text-aurora-text-muted">Last observed revision: <code>{lineage.lastObservedUpstreamRevisionId}</code></p> : null}
    </div>
  </section>
}
