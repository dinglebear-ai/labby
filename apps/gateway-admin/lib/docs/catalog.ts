const REPOSITORY_SOURCE_BASE = 'https://github.com/dinglebear-ai/labby'

export type EmbeddedDoc = {
  path: string
  title: string
  section: string
  bytes: number
  status: string | null
}

export type EmbeddedDocsManifest = {
  version: 1
  documents: EmbeddedDoc[]
}

function isSafeEmbeddedDocPath(path: string) {
  if (!path || path.startsWith('/') || path.includes('\\')) return false
  if (!path.toLowerCase().endsWith('.md')) return false

  const segments = path.split('/')
  return segments.every((segment) => segment.length > 0 && segment !== '.' && segment !== '..')
}

function isEmbeddedDoc(value: unknown): value is EmbeddedDoc {
  if (!value || typeof value !== 'object') return false
  const doc = value as Record<string, unknown>
  return (
    typeof doc.path === 'string' &&
    isSafeEmbeddedDocPath(doc.path) &&
    typeof doc.title === 'string' &&
    doc.title.trim().length > 0 &&
    typeof doc.section === 'string' &&
    doc.section.trim().length > 0 &&
    typeof doc.bytes === 'number' &&
    Number.isSafeInteger(doc.bytes) &&
    doc.bytes >= 0 &&
    (doc.status === undefined ||
      doc.status === null ||
      (typeof doc.status === 'string' &&
        doc.status.length <= 80 &&
        !/[\r\n]/.test(doc.status)))
  )
}

export function parseDocsManifest(value: unknown): EmbeddedDocsManifest | null {
  if (!value || typeof value !== 'object') return null
  const manifest = value as Record<string, unknown>
  if (manifest.version !== 1 || !Array.isArray(manifest.documents)) return null
  if (!manifest.documents.every(isEmbeddedDoc)) return null

  const paths = new Set<string>()
  for (const doc of manifest.documents) {
    if (paths.has(doc.path)) return null
    paths.add(doc.path)
  }

  return {
    version: 1,
    documents: manifest.documents.map((doc) => ({ ...doc, status: doc.status ?? null })),
  }
}

export function encodeDocAssetPath(path: string) {
  return path.split('/').map(encodeURIComponent).join('/')
}

function decodeRelativePath(path: string) {
  try {
    return path.split('/').map(decodeURIComponent).join('/')
  } catch {
    return null
  }
}

function isExternalOrAbsoluteHref(href: string) {
  return (
    href.startsWith('#') ||
    href.startsWith('/') ||
    href.startsWith('//') ||
    /^[a-z][a-z0-9+.-]*:/i.test(href)
  )
}

function resolveRelativePath(currentPath: string, targetPath: string) {
  const segments = currentPath.split('/').slice(0, -1)

  for (const segment of targetPath.split('/')) {
    if (!segment || segment === '.') continue
    if (segment === '..') {
      if (segments.length === 0) return null
      segments.pop()
      continue
    }
    segments.push(segment)
  }

  return segments.join('/')
}

export function resolveDocHref(
  currentPath: string,
  href: string,
  knownPaths: ReadonlySet<string>,
) {
  const trimmed = href.trim()
  if (!trimmed || isExternalOrAbsoluteHref(trimmed)) return href

  const hashIndex = trimmed.indexOf('#')
  const queryIndex = trimmed.indexOf('?')
  const boundaries = [hashIndex, queryIndex].filter((index) => index >= 0)
  const suffixIndex = boundaries.length === 0 ? -1 : Math.min(...boundaries)
  const rawTargetPath = suffixIndex === -1 ? trimmed : trimmed.slice(0, suffixIndex)
  const suffix = suffixIndex === -1 ? '' : trimmed.slice(suffixIndex)
  const targetPath = decodeRelativePath(rawTargetPath)
  if (!targetPath) return href

  const hasQuery = queryIndex >= 0 && (hashIndex === -1 || queryIndex < hashIndex)
  if (!hasQuery && targetPath.toLowerCase().endsWith('.md')) {
    const resolved = resolveRelativePath(currentPath, targetPath)
    if (resolved && knownPaths.has(resolved)) {
      return '/docs?doc=' + encodeURIComponent(resolved) + suffix
    }
  }

  const repositoryPath = resolveRelativePath('docs/' + currentPath, targetPath)
  if (!repositoryPath) return href

  const sourceKind = rawTargetPath.endsWith('/') ? 'tree' : 'blob'
  return (
    REPOSITORY_SOURCE_BASE +
    '/' +
    sourceKind +
    '/main/' +
    encodeDocAssetPath(repositoryPath) +
    suffix
  )
}
