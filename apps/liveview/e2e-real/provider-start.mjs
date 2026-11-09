import {spawn} from 'node:child_process'
import {mkdtempSync, mkdirSync, readFileSync, writeFileSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {join} from 'node:path'
import {createHash, randomBytes} from 'node:crypto'
import {createServer} from 'node:net'
import {provisionProviderPrincipals} from './provider-provision.mjs'
import {startProviderIssuer} from './provider-issuer.mjs'
import {startMcpServer} from './mcp-server.mjs'
import {waitForHttp} from './launcher-support.mjs'

const root = mkdtempSync(join(tmpdir(), 'labby-provider-run-'))
const binary = process.env.LABBY_E2E_BINARY
if (!binary || !binary.startsWith('/')) {
  throw new Error('An explicit absolute LABBY_E2E_BINARY is required')
}
const receipt = {
  binary,
  binarySha256: createHash('sha256').update(readFileSync(binary)).digest('hex'),
  sourceRevision: process.env.LABBY_E2E_SOURCE_REVISION,
  stages: [],
}
if (!/^[a-f0-9]{40}$/.test(receipt.sourceRevision || '')) {
  throw new Error('Explicit source provenance LABBY_E2E_SOURCE_REVISION required')
}
const children = []
const ports = [4328, 4329, 4330, 4331]
let issuer
let upstream
let stopping = false

function record(stage) {
  receipt.stages.push(stage)
  writeFileSync(join(root, 'receipt.json'), JSON.stringify(receipt, null, 2), {mode: 0o600})
}

async function admitPort(port) {
  const probe = createServer()
  await new Promise((resolve, reject) => {
    probe.once('error', reject)
    probe.listen(port, '127.0.0.1', resolve)
  })
  await new Promise(resolve => probe.close(resolve))
}

async function stopChild(child) {
  if (child.exitCode !== null || child.signalCode !== null) return
  await new Promise(resolve => {
    const timer = setTimeout(() => {
      try { process.kill(-child.pid, 'SIGKILL') } catch {}
    }, 5000)
    child.once('exit', () => {
      clearTimeout(timer)
      resolve()
    })
    try {
      process.kill(-child.pid, 'SIGTERM')
    } catch {
      clearTimeout(timer)
      resolve()
    }
  })
}

async function stop(code) {
  if (stopping) return
  stopping = true
  await Promise.all(children.map(stopChild))
  await upstream?.close()
  await issuer?.close()
  for (const port of ports) {
    try {
      await admitPort(port)
    } catch {
      code = 1
      record(`port-${port}-not-free`)
    }
  }
  record(`shutdown-${code}`)
  process.exit(code)
}

process.on('SIGINT', () => void stop(0))
process.on('SIGTERM', () => void stop(0))

function launch(command, args, env, name) {
  // Child output can contain OAuth queries or cookies. Retain stage/exit metadata only.
  const child = spawn(command, args, {env, detached: true, stdio: 'ignore'})
  children.push(child)
  child.once('error', () => void stop(1))
  child.once('exit', (code, signal) => {
    record(`${name}-exit-${code ?? signal}`)
    if (!stopping) void stop(1)
  })
  record(`${name}-pid-${child.pid}`)
}

async function main() {
  for (const port of ports) await admitPort(port)
  const ownerPassword = process.env.LABBY_E2E_PROVIDER_PASSWORD
  const memberPassword = process.env.LABBY_E2E_MEMBER_PASSWORD
  if (!ownerPassword || !memberPassword) throw new Error('Both controlled account passwords required')
  issuer = await startProviderIssuer({
    port: 4331,
    redirectUri: 'http://127.0.0.1:4329/auth/oidc/callback',
    ownerPassword,
    memberPassword,
    tlsDirectory: process.env.LABBY_E2E_PROVIDER_TLS,
  })
  record('issuer-ready')
  const home = join(root, 'labby')
  const accountHome = join(root, 'home')
  const temporary = join(root, 'tmp')
  for (const path of [home, accountHome, temporary]) mkdirSync(path, {mode: 0o700})
  writeFileSync(join(home, 'config.toml'), `config_version = 1
[code_mode]
enabled = false
[[upstream]]
name = "provider-real-mcp"
enabled = true
url = "http://127.0.0.1:4330/mcp"
`, {mode: 0o600})
  const env = {
    PATH: process.env.PATH,
    LANG: process.env.LANG || 'C.UTF-8',
    HOME: accountHome,
    LABBY_HOME: home,
    TMPDIR: temporary,
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
  upstream = await startMcpServer({port: 4330})
  launch(binary, [
    'serve', '--host', '127.0.0.1', '--port', '4329',
    '--services', 'gateway,doctor,server_logs,projects,access', '--no-input',
  ], env, 'gateway')
  await waitForHttp('http://127.0.0.1:4329/health', {stage: 'provider-gateway-health'})
  await waitForHttp('http://127.0.0.1:4329/ready', {stage: 'provider-gateway-ready'})
  record('gateway-ready')
  const provisioning = await provisionProviderPrincipals({issuer})
  record(`two-principals-${provisioning.distinctReadyPrincipals}-wrong-issuer-rejected-${provisioning.wrongIssuerRejected}`)
  launch('mix', ['run', '--no-start', 'e2e-real/provider-server.exs'], {
    PATH: process.env.PATH,
    HOME: accountHome,
    TMPDIR: temporary,
    LANG: env.LANG,
    MIX_ENV: 'test',
  }, 'console')
  await waitForHttp('http://127.0.0.1:4328/sign-in', {stage: 'provider-console-ready'})
  record('console-ready')
  await waitForHttp('http://127.0.0.1:4328/assets/js/app.js', {stage: 'provider-js-asset'})
  record('asset-ready')
  console.log(`Controlled IdP launcher ready. Private receipts ${root}`)
}

main().catch(() => {
  record('startup-failed')
  void stop(1)
})
