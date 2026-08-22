import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'

const buildId = (await readFile(new URL('../.next/BUILD_ID', import.meta.url), 'utf8')).trim()
const loadoutsTree = await readFile(
  new URL('../out/loadouts/__next._tree.txt', import.meta.url),
  'utf8',
)
const loadoutsHtml = await readFile(new URL('../out/loadouts/index.html', import.meta.url), 'utf8')

assert.ok(buildId.length > 0, 'Next must emit a non-empty build ID')
assert.match(
  loadoutsTree,
  new RegExp(`"buildId":"${buildId.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}"`),
  'the exported Loadouts navigation payload must carry the current build ID',
)
assert.doesNotMatch(
  loadoutsHtml,
  /data-dpl-id|[?&]dpl=/,
  'static exports must not use server-coordinated deployment IDs',
)

console.log(`Static navigation build ID verified: ${buildId}`)
