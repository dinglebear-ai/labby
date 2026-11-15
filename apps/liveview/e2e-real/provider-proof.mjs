import {spawn} from 'node:child_process'
import {mkdtempSync,mkdirSync,writeFileSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {join} from 'node:path'
import {randomBytes} from 'node:crypto'
import {createServer} from 'node:net'
import {startProviderIssuer} from './provider-issuer.mjs'
import {provisionProviderPrincipals} from './provider-provision.mjs'
import {validateProofSelection} from './provider-run-ownership.mjs'
import {waitForHttp} from './launcher-support.mjs'

const root = mkdtempSync(join(tmpdir(), 'labby-provider-proof-'))
const binary = process.env.LABBY_E2E_BINARY
const receipt = {binary, stages: [], binarySha256: null, sourceRevision: process.env.LABBY_E2E_SOURCE_REVISION}
let issuer
let child
let exitCode = 1
let startupDiagnostic = ''
function record(stage, detail = {}) {
  receipt.stages.push({stage, ...detail})
  writeFileSync(join(root, 'receipt.json'), JSON.stringify(receipt, null, 2), {mode: 0o600})
}
async function admit(port) {
  const probe = createServer()
  await new Promise((resolve, reject) => {
    probe.once('error', reject)
    probe.listen(port, '127.0.0.1', resolve)
  })
  await new Promise(resolve => probe.close(resolve))
}
try {
  const selection = validateProofSelection({
    binary,
    expectedSha256: process.env.LABBY_E2E_EXPECTED_BINARY_SHA256,
    sourceRevision: receipt.sourceRevision,
  })
  receipt.binarySha256 = selection.binarySha256
  await admit(4329)
  await admit(4331)
  issuer = await startProviderIssuer({port: 4331})
  const home = join(root, 'labby')
  const accountHome = join(root, 'home')
  const temporary = join(root, 'tmp')
  for (const path of [home, accountHome, temporary]) mkdirSync(path, {mode: 0o700})
  writeFileSync(join(home, 'config.toml'), 'config_version = 1\n[code_mode]\nenabled = false\n', {mode: 0o600})
  const env = {
    PATH: process.env.PATH,
    HOME: accountHome,
    TMPDIR: temporary,
    LANG: 'C.UTF-8',
    LABBY_HOME: home,
    LABBY_LOG_DIR: join(root, 'logs'),
    LABBY_AUTH_MODE: 'oauth',
    LABBY_AUTH_PROVIDER: 'authelia',
    LABBY_PUBLIC_URL: 'http://127.0.0.1:4329',
    LABBY_AUTH_DESKTOP_ORIGIN: 'http://127.0.0.1:4329',
    LABBY_AUTH_ADMIN_EMAIL: issuer.accounts[0].email,
    LABBY_AUTHELIA_ISSUER_URL: issuer.origin,
    LABBY_AUTHELIA_CLIENT_ID: issuer.clientId,
    LABBY_AUTHELIA_CLIENT_SECRET: issuer.clientSecret,
    LABBY_AUTHELIA_TRUSTED_PRIVATE_ORIGIN: issuer.origin,
    LABBY_AUTHELIA_CA_CERT_PATH: issuer.caPath,
    LABBY_TOKEN_ENCRYPTION_KEY: randomBytes(32).toString('hex'),
  }
  child = spawn(binary, ['serve', '--host', '127.0.0.1', '--port', '4329', '--services', 'gateway,doctor,server_logs,projects,access', '--no-input'], {
    env, detached: true, stdio: ['ignore', 'ignore', 'pipe'],
  })
  child.stderr.on('data', chunk => {
    startupDiagnostic = (startupDiagnostic + chunk.toString()).slice(-8192)
    for (const secret of [env.LABBY_AUTHELIA_CLIENT_SECRET, env.LABBY_TOKEN_ENCRYPTION_KEY]) {
      startupDiagnostic = startupDiagnostic.replaceAll(secret, '[REDACTED]')
    }
    startupDiagnostic = startupDiagnostic.replace(/https?:\/\/[^\s]+/g, '[URL]')
  })
  child.once('exit', (code, signal) => record('exit-observed', {code, signal}))
  child.once('error', () => record('spawn-failed'))
  record('spawn', {pid: child.pid})
  await waitForHttp('http://127.0.0.1:4329/ready', {attempts: 40, stage: 'proof-ready'})
  record('ready')
  const proof = await provisionProviderPrincipals({issuer, onReceipt: detail => record('public-contract', detail)})
  record('public-provisioning', proof)
  exitCode = 0
} catch (error) {
  record('startup-diagnostic', {text: startupDiagnostic})
  record('failed', {code: /^PROVIDER_[A-Z0-9_]+$/.test(error.message) ? error.message : 'PROVIDER_PROOF_FAILED'})
} finally {
  if (child && child.exitCode === null && child.signalCode === null) {
    await new Promise(resolve => {
      const timer = setTimeout(() => process.kill(-child.pid, 'SIGKILL'), 5000)
      child.once('exit', (code, signal) => {
        clearTimeout(timer)
        record('child-terminal', {code, signal})
        resolve()
      })
      process.kill(-child.pid, 'SIGTERM')
    })
  }
  await issuer?.close()
  try {
    await admit(4329)
    await admit(4331)
    record('ports-free')
  } catch {
    record('ports-not-free')
    exitCode = 1
  }
  record('terminal', {exitCode})
  console.log(JSON.stringify({root, ...receipt}))
  process.exitCode = exitCode
}
