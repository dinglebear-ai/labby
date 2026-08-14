import { gzipSync } from 'node:zlib'
import { readFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const maxCompressedBytes = 450 * 1024
const routes = [
  { label: '/', output: 'index' },
  { label: '/gateway', output: 'gateway' },
  { label: '/gateways', output: 'gateways' },
]

async function compressedInitialRouteBytes(output) {
  const html = await readFile(path.join(appRoot, '.next', 'server', 'app', `${output}.html`), 'utf8')
  const sources = new Set(
    [...html.matchAll(/<script[^>]+src="([^"]+\.js)"/g)].map((match) => match[1]),
  )
  let total = 0
  for (const source of sources) {
    const relative = source.replace(/^\/_next\//, '')
    const body = await readFile(path.join(appRoot, '.next', relative))
    total += gzipSync(body).byteLength
  }
  return { total, chunks: sources.size }
}

for (const route of routes) {
  const { total, chunks } = await compressedInitialRouteBytes(route.output)
  if (total > maxCompressedBytes) {
    throw new Error(
      `${route.label} initial JavaScript is ${(total / 1024).toFixed(1)} KiB compressed across ${chunks} chunks; budget is ${maxCompressedBytes / 1024} KiB`,
    )
  }
  console.log(`${route.label}: ${(total / 1024).toFixed(1)} KiB compressed (${chunks} initial chunks)`)
}
