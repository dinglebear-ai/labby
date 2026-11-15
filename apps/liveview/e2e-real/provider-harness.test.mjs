import {test} from 'node:test'
import assert from 'node:assert/strict'
import {execFileSync} from 'node:child_process'
import {mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync, mkdirSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {dirname, join, resolve, delimiter} from 'node:path'
import {fileURLToPath} from 'node:url'
import {selectProviderOutput, cleanupOwnedProviderOutput} from './provider-run-output.mjs'
import {prepareProviderTls} from './provider-issuer.mjs'

const directory = dirname(fileURLToPath(import.meta.url))
const application = resolve(directory, '..')

test('normal npm E2E entrypoint prepares assets and the required test build before Node fixtures', () => {
  const manifest = JSON.parse(readFileSync(join(application, 'package.json'), 'utf8'))
  assert.equal(manifest.scripts['test:e2e'],
    'playwright test --config=playwright.config.mjs && playwright test --config=playwright.provider.config.mjs')
  const fixture = mkdtempSync(join(tmpdir(), 'labby-pretest-order-'))
  try {
    const suite = join(fixture, 'e2e-real')
    const bin = join(fixture, 'bin')
    mkdirSync(suite)
    mkdirSync(bin)
    writeFileSync(join(suite, 'build.mjs'), readFileSync(join(directory, 'build.mjs')))
    writeFileSync(join(bin, 'mix'), `#!/usr/bin/env node
import {existsSync, writeFileSync} from 'node:fs'
import assert from 'node:assert/strict'
if (process.argv[2] === 'assets.deploy') {
  assert.equal(process.env.MIX_ENV, undefined)
  writeFileSync('assets-ready', '')
} else if (process.argv[2] === 'compile') {
  assert.equal(process.env.MIX_ENV, 'test')
  assert.equal(existsSync('assets-ready'), true)
  if (process.env.CONTROLLED_TEST_COMPILE_FAILURE) process.exit(23)
  writeFileSync('test-build-ready', '')
} else process.exit(24)
`, {mode: 0o700})
    writeFileSync(join(suite, 'browser.test.mjs'), `
import {test} from 'node:test'
import assert from 'node:assert/strict'
import {existsSync, writeFileSync} from 'node:fs'
test('assets and test modules precede every Node fixture', () => {
  assert.equal(existsSync('assets-ready'), true)
  assert.equal(existsSync('test-build-ready'), true)
  writeFileSync('node-fixture-ran', '')
})
`)
    const env = {...process.env, PATH: bin + delimiter + process.env.PATH,
      LABBY_E2E_BINARY: '/controlled/provided-binary'}
    for (const key of ['NODE_TEST_CONTEXT', 'MIX_ENV', 'MIX_BUILD_PATH']) delete env[key]
    execFileSync('/bin/sh', ['-c', manifest.scripts['pretest:e2e']],
      {cwd: fixture, env, timeout: 10000, stdio: 'pipe'})
    assert.equal(readFileSync(join(fixture, 'node-fixture-ran'), 'utf8'), '')
    rmSync(join(fixture, 'test-build-ready'))
    rmSync(join(fixture, 'node-fixture-ran'))
    let failed
    try {
      execFileSync('/bin/sh', ['-c', manifest.scripts['pretest:e2e']], {
        cwd: fixture, env: {...env, CONTROLLED_TEST_COMPILE_FAILURE: '1'}, timeout: 10000, stdio: 'pipe',
      })
    } catch (error) { failed = error.status }
    assert.equal(failed, 23)
    assert.equal(readdirSync(fixture).includes('node-fixture-ran'), false)
  } finally {
    rmSync(fixture, {recursive: true, force: true})
  }
  const lock = JSON.parse(readFileSync(join(application, 'package-lock.json'), 'utf8'))
  assert.equal(lock.packages[''].devDependencies['@playwright/test'], manifest.devDependencies['@playwright/test'])
})

test('every real browser spec belongs to exactly one discovered suite with no silent skips', () => {
  const tls = prepareProviderTls()
  const restricted = mkdtempSync(join(tmpdir(), 'labby-provider-discovery-'))
  const env = {
    ...process.env,
    LABBY_E2E_PROVIDER_TLS: tls.root,
    LABBY_E2E_PROVIDER_SPKI: tls.spki,
    LABBY_E2E_RESTRICTED_ROOT: restricted,
  }
  const providerOutputOwner = selectProviderOutput(env)
  function discover(config) {
    const output = execFileSync(process.execPath, [
      join(application, 'node_modules/@playwright/test/cli.js'),
      'test', '--list', '--reporter=json', `--config=${config}`,
    ], {cwd: application, env, timeout: 15000, maxBuffer: 4194304, encoding: 'utf8'})
    const report = JSON.parse(output)
    assert.deepEqual(report.errors, [])
    const files = new Set()
    function visit(suites) {
      for (const suite of suites) {
        if (suite.specs?.length) files.add(suite.file)
        visit(suite.suites || [])
      }
    }
    visit(report.suites)
    return files
  }
  try {
    const regular = discover('playwright.config.mjs')
    const provider = discover('playwright.provider.config.mjs')
    const files = readdirSync(directory).filter(file => file.endsWith('.spec.mjs'))
    assert.ok(regular.size > 0)
    assert.ok(provider.size > 0)
    for (const file of files) {
      const count = Number(regular.has(file)) + Number(provider.has(file))
      assert.equal(count, 1, `${file} must be discovered exactly once`)
      assert.equal(provider.has(file), file.startsWith('provider-'), `${file} selected by wrong harness`)
      assert.doesNotMatch(readFileSync(join(directory, file), 'utf8'), /test\.(?:skip|fixme)\s*\(/)
    }
    console.log(`Discovered ${regular.size} static and ${provider.size} provider specs`)
  } finally {
    assert.equal(cleanupOwnedProviderOutput(providerOutputOwner, env), true)
    rmSync(tls.root, {recursive: true, force: true})
    rmSync(restricted, {recursive: true, force: true})
  }
})
