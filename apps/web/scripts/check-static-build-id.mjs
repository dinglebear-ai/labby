import { readFile } from 'node:fs/promises'
import { verifyStaticNavigationBuildId } from './static-navigation-contract.mjs'

const buildId = (await readFile(new URL('../.next/BUILD_ID', import.meta.url), 'utf8')).trim()
const tree = await readFile(new URL('../out/loadouts/__next._tree.txt', import.meta.url), 'utf8')
const html = await readFile(new URL('../out/loadouts/index.html', import.meta.url), 'utf8')
verifyStaticNavigationBuildId({ buildId, tree, html })
console.log(`Static navigation build ID verified: ${buildId}`)
