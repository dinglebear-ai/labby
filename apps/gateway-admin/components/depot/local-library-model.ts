import type { DepotArtifact, FederatedArtifact } from '@/lib/api/depot-client'
import type { SkillLibraryItem } from '@/lib/api/skill-library-client'

/** Presentation projection only; acquisition identity and access remain backend-owned. */
export function localLibraryArtifact(item: SkillLibraryItem): DepotArtifact {
  return {
    id: item.artifact_id,
    name: item.name,
    kind: 'skill',
    namespace: item.access_label,
    currentRevisionId: item.latest_revision_id,
    currentRevision: {
      id: item.latest_revision_id,
      components: item.latest_revision_files.map(file => ({ path: file.path, mediaType: file.media_type ?? undefined, size: file.size })),
    },
    publication: { visibility: item.visibility },
  }
}

/** Adapts a Labby-owned Library record to the shared Discover inspection surface. */
export function localLibraryFederatedArtifact(artifact: DepotArtifact): FederatedArtifact {
  const artifactId = artifact.id ?? artifact.descriptor?.id ?? 'unknown'
  const kind = artifact.kind ?? artifact.descriptor?.kind ?? 'artifact'
  const revisionId = artifact.currentRevisionId ?? artifact.currentRevision?.id
  return {
    providerId: 'labby',
    artifactId,
    id: artifactId,
    kind,
    namespace: artifact.namespace ?? artifact.descriptor?.namespace,
    name: artifact.name ?? artifact.descriptor?.name,
    title: artifact.title ?? artifact.descriptor?.title,
    description: artifact.description ?? artifact.descriptor?.description,
    currentRevisionId: revisionId,
    contentDigest: artifact.contentDigest ?? artifact.currentRevision?.contentDigest,
    revisionCount: artifact.revisionCount,
    sourceOrigin: artifact.sourceOrigin as FederatedArtifact['sourceOrigin'],
    publisherVerified: artifact.publisherVerified,
    metrics: artifact.metrics,
    upstreamBehind: artifact.upstreamBehind,
    updatedLabel: artifact.updatedLabel,
    publication: artifact.publication,
    license: artifact.license ? {
      declared: artifact.license.declared,
      redistribution: artifact.license.redistribution,
      reviewState: artifact.license.reviewState,
      takedownState: artifact.license.takedownState,
    } : undefined,
    provenance: artifact.provenance,
    descriptor: {
      id: artifact.descriptor?.id ?? artifactId,
      kind,
      namespace: artifact.descriptor?.namespace ?? artifact.namespace,
      name: artifact.descriptor?.name ?? artifact.name,
      title: artifact.descriptor?.title ?? artifact.title,
      description: artifact.descriptor?.description ?? artifact.description,
      tags: artifact.descriptor?.tags,
    },
    currentRevision: revisionId || artifact.currentRevision?.components?.length ? {
      id: revisionId,
      contentDigest: artifact.currentRevision?.contentDigest ?? artifact.contentDigest,
      authoredAt: artifact.currentRevision?.createdAt,
      fileCount: artifact.currentRevision?.fileCount ?? artifact.currentRevision?.components?.length,
    } : undefined,
    readme: artifact.readme ?? { state: 'unavailable', reason: 'absent' },
  }
}
