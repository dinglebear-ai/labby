import path from 'node:path'
import { execFileSync } from 'node:child_process'
import { randomUUID } from 'node:crypto'
import { realpathSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

/** @type {import('next').NextConfig} */
const allowedDevOrigins = ['127.0.0.1', 'localhost']
const dirname = path.dirname(fileURLToPath(import.meta.url))

export function resolveBuildId({
  environment = process.env,
  readGitRevision = () => execFileSync(
    'git',
    ['rev-parse', 'HEAD'],
    { cwd: dirname, encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] },
  ),
  createFallbackId = () => `archive-${randomUUID()}`,
} = {}) {
  const explicitBuildId = environment.NEXT_BUILD_ID
  if (explicitBuildId !== undefined) {
    if (!/^[A-Za-z0-9_-]+$/.test(explicitBuildId)) {
      throw new Error('NEXT_BUILD_ID must contain only letters, numbers, hyphens, and underscores')
    }
    return explicitBuildId
  }

  try {
    const revision = readGitRevision().trim()
    if (revision.length > 0) return revision
  } catch {
    // Source archives and minimal builders may not include Git metadata.
  }

  return createFallbackId()
}

// Managed worktrees may reuse dependencies through a symlink outside the app.
// Turbopack needs both the source and resolved dependencies inside its root.
export function resolveTurbopackRoot(appDirectory, resolveDependencies = () => realpathSync(path.join(appDirectory, 'node_modules'))) {
  let dependencies
  try {
    dependencies = resolveDependencies()
  } catch (error) {
    if (error?.code === 'ENOENT') return appDirectory
    throw new Error('Cannot resolve Gateway Admin dependencies for Turbopack', { cause: error })
  }
  let root = path.resolve(appDirectory)
  while (true) {
    const relative = path.relative(root, dependencies)
    if (!path.isAbsolute(relative) && relative !== '..' && !relative.startsWith(`..${path.sep}`)) return root
    const parent = path.dirname(root)
    if (parent === root || parent === path.parse(parent).root) {
      throw new Error('Gateway Admin source and linked dependencies require a filesystem-wide Turbopack root; use dependencies under a shared project directory')
    }
    root = parent
  }
}

const buildId = resolveBuildId()
const assetPrefix = process.env.LABBY_ASSET_PREFIX?.trim() || undefined

if (process.env.LAB_ALLOWED_DEV_ORIGINS) {
  for (const origin of process.env.LAB_ALLOWED_DEV_ORIGINS.split(',')) {
    const trimmed = origin.trim()
    if (trimmed.length > 0) {
      allowedDevOrigins.push(trimmed)
    }
  }
}

const nextConfig = {
  output: 'export',
  assetPrefix,
  generateBuildId: async () => buildId,
  turbopack: {
    root: resolveTurbopackRoot(dirname),
  },
  trailingSlash: true,
  allowedDevOrigins,
  images: {
    unoptimized: true,
  },
}

export default nextConfig
