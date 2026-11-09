import {test} from 'node:test'
import assert from 'node:assert/strict'
import {randomBytes} from 'node:crypto'
import {mkdtempSync, mkdirSync, writeFileSync, readFileSync, readdirSync, rmSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {join, resolve} from 'node:path'
import {spawnSync} from 'node:child_process'
import {createRequire} from 'node:module'

const require = createRequire(import.meta.url)

function artifactContents(directory) {
  return readdirSync(directory, {withFileTypes: true}).flatMap(entry => {
    const path = join(directory, entry.name)
    return entry.isDirectory() ? artifactContents(path) : [readFileSync(path)]
  })
}

test('pinned Playwright supports suppressing automatic failure page snapshots', () => {
  assert.equal(require('playwright/package.json').version, '1.59.1')
  const source = readFileSync(join(require.resolve('playwright/package.json'), '..', 'lib', 'index.js'), 'utf8')
  assert.match(source, /async _takePageSnapshot\(context\) \{\s*if \(process\.env\.PLAYWRIGHT_NO_COPY_PROMPT\)\s*return;/)
})

for (const configName of ['playwright.config.mjs', 'playwright.provider.config.mjs']) {
  test(`${configName} keeps secret-filled browser failures out of automatic artifacts and logs`, {timeout: 45000}, () => {
    const root = mkdtempSync(join(tmpdir(), 'labby-private-failure-'))
    const secret = randomBytes(24).toString('hex')
    const output = join(root, 'output')
    mkdirSync(output)
    writeFileSync(join(root, 'failure.spec.mjs'), `
import {test} from ${JSON.stringify(join(require.resolve('@playwright/test/package.json'), '..', 'index.mjs'))}
import {runPrivateScenario} from ${JSON.stringify(resolve('e2e-real/private-scenario.mjs'))}
test('sanitized private failure', async ({page}, info) => {
  await page.setContent('<label>Token<input type="password"></label><label>Private map<textarea></textarea></label>')
  await page.getByLabel('Token').fill(process.env.PRIVATE_FAILURE_SECRET)
  await page.getByLabel('Private map').fill(process.env.PRIVATE_FAILURE_SECRET)
  await runPrivateScenario({page, info, accounting: {}, cleanup: async () => {}}, async step => {
    await step('private-filled-failure', () => page.getByRole('button', {name: 'unavailable-fixture-action'}).click({timeout: 250}))
  })
})
`)
    writeFileSync(join(root, 'playwright.config.mjs'), `
import normal from ${JSON.stringify(resolve(configName))}
export default {
  testDir: ${JSON.stringify(root)}, testMatch: 'failure.spec.mjs',
  outputDir: ${JSON.stringify(output)}, workers: 1, retries: 0, timeout: 10000,
  use: normal.use,
}
`)
    const env = {...process.env, PRIVATE_FAILURE_SECRET: secret,
      LABBY_E2E_PRIVATE_ROOT: root, LABBY_E2E_RESTRICTED_ROOT: root,
      LABBY_E2E_PROVIDER_TLS: root, LABBY_E2E_PROVIDER_SPKI: `${'a'.repeat(43)}=`,
      PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH || '/usr/bin/google-chrome'}
    delete env.PLAYWRIGHT_NO_COPY_PROMPT
    try {
      const result = spawnSync(process.execPath, [require.resolve('@playwright/test/cli'), 'test', '--config', join(root, 'playwright.config.mjs')], {
        cwd: process.cwd(), env, timeout: 30000, encoding: 'utf8', maxBuffer: 262144,
      })
      assert.equal(result.status, 1, 'The deliberate browser failure must run and exit 1')
      assert.match(result.stdout, /PRIVATE_SCENARIO_FAILED stage=private-filled-failure kind=timeout/)
      const contents = [Buffer.from(result.stdout || ''), Buffer.from(result.stderr || ''), ...artifactContents(output)]
      assert.equal(contents.some(content => content.includes(secret)), false,
        'Automatic artifacts and process output must exclude the private input')
      assert.equal(readdirSync(output, {recursive: true}).some(name => /\.(png|webm|zip)$/.test(name)), false)
    } finally {
      rmSync(root, {recursive: true, force: true})
    }
  })
}
