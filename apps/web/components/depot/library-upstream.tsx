import Link from 'next/link'
import type { DepotArtifact } from '@/lib/api/depot-client'
import { AURORA_MUTED_LABEL } from '@/components/aurora/tokens'
import { libraryUpstreamHref } from './local-library-model'

export function LibraryUpstream({ artifact }: { artifact: DepotArtifact }) {
  const lineage = artifact.lineage
  if (!lineage?.upstreamArtifactId && !lineage?.forkedFromArtifactId) return null
  return <section aria-label="Upstream" className="rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-medium p-4 text-xs">
    <h3 className={AURORA_MUTED_LABEL}>Upstream</h3>
    {[["Upstream artifact", lineage.upstreamArtifactId], ["Forked from", lineage.forkedFromArtifactId]].map(([label, id]) => {
      if (!id) return null
      const href = libraryUpstreamHref(artifact, id)
      return <p key={label} className="mt-2 break-all">{label}: {href ? <Link className="text-aurora-accent-primary underline" href={href}>{id}</Link> : id}</p>
    })}
    {lineage.upstreamArtifactId ? <p className="mt-2 text-aurora-text-muted">{lineage.following === undefined ? 'Following status not supplied.' : lineage.following ? 'Following upstream.' : 'Not following upstream.'} This does not indicate synchronization status.</p> : null}
  </section>
}
