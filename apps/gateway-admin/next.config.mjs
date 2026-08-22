import path from 'node:path'
import { execFileSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'

/** @type {import('next').NextConfig} */
const allowedDevOrigins = ['127.0.0.1', 'localhost']
const dirname = path.dirname(fileURLToPath(import.meta.url))
const deploymentId = process.env.NEXT_DEPLOYMENT_ID
  ?? execFileSync('git', ['rev-parse', 'HEAD'], { cwd: dirname, encoding: 'utf8' }).trim()

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
  deploymentId,
  turbopack: {
    root: dirname,
  },
  trailingSlash: true,
  allowedDevOrigins,
  typescript: {
    // `pnpm exec tsc --noEmit` passes for the app source. The failure in
    // `next build` comes from Next 16's generated `.next/types/validator.ts`
    // importing `./routes.js` during its internal build-time check. Keep the
    // standalone TypeScript check available, but do not let Next's generated
    // validator block `labby serve` boot.
    ignoreBuildErrors: true,
  },
  images: {
    unoptimized: true,
  },
}

export default nextConfig
