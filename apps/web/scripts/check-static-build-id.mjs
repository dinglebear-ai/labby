import { readFile } from 'node:fs/promises'
import { assertStaticNavigationBuildId } from './static-build-id-contract.mjs'

const buildId = (await readFile(new URL('../.next/BUILD_ID', import.meta.url), 'utf8')).trim()
const loadoutsTree = await readFile(new URL('../out/loadouts/__next._tree.txt', import.meta.url), 'utf8')
const loadoutsHtml = await readFile(new URL('../out/loadouts/index.html', import.meta.url), 'utf8')

assertStaticNavigationBuildId(buildId, loadoutsTree, loadoutsHtml)
console.log(`Static navigation build ID verified: ${buildId}`)
