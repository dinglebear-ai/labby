import {test} from 'node:test'
import assert from 'node:assert/strict'
import {execFileSync} from 'node:child_process'
import {mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync, mkdirSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {dirname, join, resolve} from 'node:path'
import {fileURLToPath} from 'node:url'
import {prepareProviderTls} from './provider-issuer.mjs'

const directory = dirname(fileURLToPath(import.meta.url))
const application = resolve(directory, '..')

test('normal npm E2E entrypoint runs both real suites after existing pretest build', () => {
  const manifest = JSON.parse(readFileSync(join(application, 'package.json'), 'utf8'))
  assert.equal(manifest.scripts['test:e2e'],
    'playwright test --config=playwright.config.mjs && playwright test --config=playwright.provider.config.mjs')
  const fixture = mkdtempSync(join(tmpdir(), 'labby-pretest-order-'))
  try {
    const suite = join(fixture, 'e2e-real')
    mkdirSync(suite)
    writeFileSync(join(suite, 'build.mjs'), "import {writeFileSync} from 'node:fs'; writeFileSync('assets-ready', '')")
    writeFileSync(join(suite, 'browser.test.mjs'), "import {test} from 'node:test'; import assert from 'node:assert/strict'; import {existsSync} from 'node:fs'; test('assets precede browser helpers', () => assert.equal(existsSync('assets-ready'), true))")
    const env = {...process.env}
    delete env.NODE_TEST_CONTEXT
    execFileSync('/bin/sh', ['-c', manifest.scripts['pretest:e2e']], {cwd: fixture, env, timeout: 10000, stdio: 'pipe'})
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
    rmSync(tls.root, {recursive: true, force: true})
    rmSync(restricted, {recursive: true, force: true})
  }
})
