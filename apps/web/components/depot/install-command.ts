import type { FederatedArtifact } from '@/lib/api/depot-client'

/** Keep a catalog reference one POSIX shell argument, including embedded quotes. */
export function artifactInstallCommand(artifact: FederatedArtifact): string | undefined {
  const namespace = artifact.namespace ?? artifact.descriptor?.namespace
  const name = artifact.name ?? artifact.descriptor?.name
  if (!namespace || !name) return undefined
  const reference = namespace.includes('/') ? namespace : `${namespace}/${name}`
  if (reference.length > 1024 || Array.from(reference).some(character => character.charCodeAt(0) < 32 || character.charCodeAt(0) === 127)) return undefined
  return `depot add '${reference.replaceAll("'", "'\\''")}'`
}
