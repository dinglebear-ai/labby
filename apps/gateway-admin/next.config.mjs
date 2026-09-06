import path from 'node:path'
import { execFileSync } from 'node:child_process'
import { randomUUID } from 'node:crypto'
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

const buildId = resolveBuildId()

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
  generateBuildId: async () => buildId,
  turbopack: {
    root: dirname,
  },
  trailingSlash: true,
  allowedDevOrigins,
  images: {
    unoptimized: true,
  },
}

export default nextConfig
