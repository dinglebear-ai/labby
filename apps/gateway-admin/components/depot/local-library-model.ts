import type { DepotArtifact } from '@/lib/api/depot-client'
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
