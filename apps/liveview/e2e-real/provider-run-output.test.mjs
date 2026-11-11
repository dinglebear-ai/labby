import {test} from 'node:test'
import assert from 'node:assert/strict'
import {execFileSync, spawnSync} from 'node:child_process'
import {mkdtempSync, mkdirSync, writeFileSync, readFileSync, readdirSync, chmodSync, rmSync, lstatSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {join, resolve} from 'node:path'
import {pathToFileURL} from 'node:url'
import {selectProviderOutput, validateProviderOutput, cleanupOwnedProviderOutput} from './provider-run-output.mjs'

const application = resolve(import.meta.dirname, '..')
const normalConfig = pathToFileURL(join(application, 'playwright.provider.config.mjs')).href
const playwright = pathToFileURL(join(application, 'node_modules/@playwright/test/index.mjs')).href
const envKeys = ['LABBY_E2E_PROVIDER_RUN_ID', 'LABBY_E2E_PROVIDER_OUTPUT_ROOT', 'LABBY_E2E_PROVIDER_OUTPUT_NONCE']

function fixture() {
  const root = mkdtempSync(join(tmpdir(), 'labby-output-regression-'))
  const tls = join(root, 'external-tls')
  mkdirSync(tls)
  writeFileSync(join(tls, 'certificate-input'), 'untouched external certificate bytes', {mode: 0o400})
  const env = {...process.env, LABBY_E2E_PROVIDER_TLS: tls,
    LABBY_E2E_PROVIDER_SPKI: `${'a'.repeat(43)}=`}
  for (const key of [...envKeys, 'NODE_TEST_CONTEXT']) delete env[key]
  return {root, tls, env}
}

function snapshot(root) {
  return readdirSync(root).sort().map(name => [name, readFileSync(join(root, name), 'utf8')])
}

test('normal provider config preserves the owning runner identity in a real Playwright worker', {timeout: 20000}, () => {
  const {root, tls, env} = fixture()
  try {
    writeFileSync(join(root, 'launcher.mjs'), `
import {writeProviderAccessManifest} from ${JSON.stringify(pathToFileURL(join(application, 'e2e-real/provider-run-output.mjs')).href)}
writeProviderAccessManifest(process.env, {runId: process.env.LABBY_E2E_PROVIDER_RUN_ID, marker: 'launcher-metadata-only'})
`)
    writeFileSync(join(root, 'worker.spec.mjs'), `
import {test, expect} from ${JSON.stringify(playwright)}
test('worker uses the runner output identity', async () => {
  expect(process.env.LABBY_E2E_PROVIDER_RUN_ID).toBe(process.env.PROVIDER_RUNNER_EXPECTED_ID)
  expect(process.env.LABBY_E2E_PROVIDER_OUTPUT_ROOT).toBe(process.env.PROVIDER_RUNNER_EXPECTED_ROOT)
  const output = await import(${JSON.stringify(pathToFileURL(join(application, 'e2e-real/provider-run-output.mjs')).href)})
  expect(output.readProviderAccessManifest()).toEqual({runId: process.env.PROVIDER_RUNNER_EXPECTED_ID, marker: 'launcher-metadata-only'})
})
`)
    writeFileSync(join(root, 'config.mjs'), `
import normal from ${JSON.stringify(normalConfig)}
import {execFileSync} from 'node:child_process'
if (normal.metadata?.providerOutputOwner?.pid === process.pid) {
  execFileSync(process.execPath, [${JSON.stringify(join(root, 'launcher.mjs'))}], {env: process.env, timeout: 5000})
}
process.env.PROVIDER_RUNNER_EXPECTED_ID ||= process.env.LABBY_E2E_PROVIDER_RUN_ID
process.env.PROVIDER_RUNNER_EXPECTED_ROOT ||= process.env.LABBY_E2E_PROVIDER_OUTPUT_ROOT
export default {...normal, testDir: ${JSON.stringify(root)}, testMatch: 'worker.spec.mjs',
  outputDir: ${JSON.stringify(join(root, 'results'))}, webServer: undefined,
  globalTeardown: ${JSON.stringify(join(application, 'e2e-real/provider-teardown.mjs'))}, use: {}}
`)
    const result = spawnSync(process.execPath, [join(application, 'node_modules/@playwright/test/cli.js'),
      'test', '--config', join(root, 'config.mjs'), '--reporter=line'],
    {cwd: application, env, encoding: 'utf8', timeout: 15000, maxBuffer: 262144})
    assert.equal(result.status, 0, result.stdout + result.stderr)
    assert.match(result.stdout, /1 passed/)
    assert.deepEqual(snapshot(tls), [['certificate-input', 'untouched external certificate bytes']])
  } finally {
    rmSync(root, {recursive: true, force: true})
  }
})

for (const readonly of [false, true]) {
  test(`normal provider output leaves ${readonly ? 'read-only' : 'writable shared'} TLS input unchanged`, () => {
    const {root, tls, env} = fixture()
    const before = snapshot(tls)
    const beforeMode = lstatSync(tls).mode & 0o777
    if (readonly) chmodSync(tls, 0o500)
    try {
      const result = execFileSync(process.execPath, ['--input-type=module', '-e', `
import assert from 'node:assert/strict'
import {readFileSync, existsSync} from 'node:fs'
const {default: config} = await import(${JSON.stringify(normalConfig)})
assert.match(process.env.LABBY_E2E_PROVIDER_OUTPUT_ROOT || '', /^\\//, 'output root is required')
assert.notEqual(process.env.LABBY_E2E_PROVIDER_OUTPUT_ROOT, process.env.LABBY_E2E_PROVIDER_TLS)
const output = await import(${JSON.stringify(pathToFileURL(join(application, 'e2e-real/provider-run-output.mjs')).href)})
const proof = {runId: process.env.LABBY_E2E_PROVIDER_RUN_ID, accessActions: ['access.team.list']}
const path = output.writeProviderAccessManifest(process.env, proof)
assert.deepEqual(JSON.parse(readFileSync(path, 'utf8')), proof)
const {default: teardown} = await import(${JSON.stringify(pathToFileURL(join(application, 'e2e-real/provider-teardown.mjs')).href)})
const outputRoot = process.env.LABBY_E2E_PROVIDER_OUTPUT_ROOT
await teardown(config)
assert.equal(existsSync(outputRoot), false)
assert.equal(existsSync(process.env.LABBY_E2E_PROVIDER_TLS), true)
console.log('OUTPUT_TLS_BOUNDARY_PASSED')
`], {cwd: application, env, timeout: 10000, maxBuffer: 262144, encoding: 'utf8'})
      assert.match(result, /OUTPUT_TLS_BOUNDARY_PASSED/)
      assert.deepEqual(snapshot(tls), before)
      assert.equal(lstatSync(tls).mode & 0o777, readonly ? 0o500 : beforeMode)
    } finally {
      chmodSync(tls, 0o700)
      rmSync(root, {recursive: true, force: true})
    }
  })
}


test('partial, altered and stale inherited output selections fail without cleanup of foreign input', () => {
  const env = {}
  const owner = selectProviderOutput(env)
  const marker = join(owner.root, '.labby-provider-output-owner.json')
  try {
    assert.equal(lstatSync(owner.root).mode & 0o777, 0o700)
    assert.deepEqual(selectProviderOutput(env), owner)
    assert.throws(() => selectProviderOutput({LABBY_E2E_PROVIDER_RUN_ID: owner.runId}), /PROVIDER_RUN_OUTPUT_INVALID/)
    assert.throws(() => validateProviderOutput({...env, LABBY_E2E_PROVIDER_OUTPUT_NONCE: 'a'.repeat(64)}), /PROVIDER_RUN_OUTPUT_INVALID/)
    writeFileSync(marker, JSON.stringify({...owner, pid: process.pid + 1000000}))
    assert.throws(() => selectProviderOutput(env), /PROVIDER_RUN_OUTPUT_INVALID/)
    assert.equal(cleanupOwnedProviderOutput(owner, env), false)
  } finally {
    writeFileSync(marker, JSON.stringify(owner))
    assert.equal(cleanupOwnedProviderOutput(owner, env), true)
  }
})
