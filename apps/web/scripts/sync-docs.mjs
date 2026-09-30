import { mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

const SCRIPT_PATH = fileURLToPath(import.meta.url)
const APP_DIR = resolve(dirname(SCRIPT_PATH), '..')
const REPO_ROOT = resolve(APP_DIR, '../..')
const DEFAULT_SOURCE_DIR = join(REPO_ROOT, 'docs')
const DEFAULT_OUTPUT_DIR = join(APP_DIR, 'public', '_docs')

const EXCLUDED_TOP_LEVEL_DIRECTORIES = new Set(['archive', 'sessions', 'superpowers'])
const EXCLUDED_BASENAMES = new Set(['AGENTS.md', 'CLAUDE.md', 'GEMINI.md'])

function normalizeDocPath(value) {
  return value.replaceAll('\\', '/').replace(/^\.\//, '')
}

export function shouldIncludeDoc(relativePath) {
  const normalized = normalizeDocPath(relativePath)
  const segments = normalized.split('/')
  if (
    !normalized ||
    normalized.startsWith('/') ||
    /^[a-z]:\//i.test(normalized) ||
    segments.some((segment) => !segment || segment === '.' || segment === '..')
  ) return false
  if (!normalized.toLowerCase().endsWith('.md')) return false

  const [topLevel] = segments
  if (EXCLUDED_TOP_LEVEL_DIRECTORIES.has(topLevel)) return false
  if (EXCLUDED_BASENAMES.has(basename(normalized))) return false

  return true
}

function titleCase(value) {
  return value
    .split(/[-_]+/)
    .filter(Boolean)
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
    .join(' ')
}

export function titleFromMarkdown(markdown, relativePath) {
  const heading = markdown.match(/^#\s+(.+?)\s*$/m)?.[1]?.trim()
  if (heading) return heading

  const fallback = basename(relativePath, '.md')
  return fallback.toLowerCase() === 'readme' ? 'Overview' : titleCase(fallback)
}

export function sectionForPath(relativePath) {
  const normalized = normalizeDocPath(relativePath)
  const segments = normalized.split('/')
  return segments.length === 1 ? 'Overview' : titleCase(segments[0])
}

export function statusFromMarkdown(markdown, relativePath) {
  const frontmatter = markdown.match(/^---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/)?.[1]
  const rawStatus = frontmatter?.match(/^status:\s*(.+?)\s*$/m)?.[1]?.trim()
  if (rawStatus) {
    const first = rawStatus[0]
    const last = rawStatus.at(-1)
    const unquoted =
      rawStatus.length >= 2 && ((first === '"' && last === '"') || (first === "'" && last === "'"))
        ? rawStatus.slice(1, -1).trim()
        : rawStatus
    if (unquoted.length > 80) {
      throw new Error('Document status exceeds 80 characters: ' + relativePath)
    }
    return unquoted || null
  }

  const normalized = normalizeDocPath(relativePath)
  if (normalized.startsWith('generated/')) return 'generated'
  if (normalized.startsWith('plans/')) return 'plan'
  if (normalized.startsWith('features/')) return 'feature'
  return null
}

function isWithin(parent, child) {
  const relation = relative(parent, child)
  return relation === '' || (relation !== '..' && !relation.startsWith('..' + sep) && !isAbsolute(relation))
}

export function assertNonOverlappingDocsPaths(sourceDir, outputDir) {
  const source = resolve(sourceDir)
  const output = resolve(outputDir)
  if (isWithin(source, output) || isWithin(output, source)) {
    throw new Error('Documentation source and output directories must not overlap')
  }
}

function compareDocuments(a, b) {
  if (a.path === 'README.md') return -1
  if (b.path === 'README.md') return 1
  return a.path < b.path ? -1 : a.path > b.path ? 1 : 0
}

async function collectMarkdownFiles(sourceDir, currentDir = sourceDir, relativeDir = '') {
  const entries = await readdir(currentDir, { withFileTypes: true })
  entries.sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0))

  const files = []
  for (const entry of entries) {
    const relativePath = normalizeDocPath(relativeDir ? relativeDir + '/' + entry.name : entry.name)
    const absolutePath = join(currentDir, entry.name)

    if (entry.isDirectory()) {
      const [topLevel] = relativePath.split('/')
      if (EXCLUDED_TOP_LEVEL_DIRECTORIES.has(topLevel)) continue
      files.push(...await collectMarkdownFiles(sourceDir, absolutePath, relativePath))
      continue
    }

    if (entry.isFile() && shouldIncludeDoc(relativePath)) {
      files.push({ absolutePath, relativePath })
    }
  }

  return files
}

export async function syncDocs({
  sourceDir = DEFAULT_SOURCE_DIR,
  outputDir = DEFAULT_OUTPUT_DIR,
} = {}) {
  assertNonOverlappingDocsPaths(sourceDir, outputDir)
  await rm(outputDir, { recursive: true, force: true })
  await mkdir(outputDir, { recursive: true })

  const sourceFiles = await collectMarkdownFiles(sourceDir)
  const documents = []
  let totalBytes = 0

  for (const { absolutePath, relativePath } of sourceFiles) {
    const markdown = await readFile(absolutePath, 'utf8')
    const destination = join(outputDir, ...relativePath.split('/'))
    await mkdir(dirname(destination), { recursive: true })
    await writeFile(destination, markdown, 'utf8')

    const bytes = Buffer.byteLength(markdown)
    totalBytes += bytes
    documents.push({
      path: relativePath,
      title: titleFromMarkdown(markdown, relativePath),
      section: sectionForPath(relativePath),
      bytes,
      status: statusFromMarkdown(markdown, relativePath),
    })
  }

  documents.sort(compareDocuments)
  const manifest = { version: 1, documents }
  await writeFile(
    join(outputDir, 'manifest.json'),
    JSON.stringify(manifest, null, 2) + '\n',
    'utf8',
  )

  return { documents: documents.length, totalBytes }
}

const isMain = process.argv[1] && resolve(process.argv[1]) === SCRIPT_PATH
if (isMain) {
  syncDocs()
    .then(({ documents, totalBytes }) => {
      console.log('Synced ' + documents + ' embedded docs (' + totalBytes + ' bytes).')
    })
    .catch((error) => {
      console.error(error)
      process.exitCode = 1
    })
}
