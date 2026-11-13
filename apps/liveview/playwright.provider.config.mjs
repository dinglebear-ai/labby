import {defineConfig} from '@playwright/test'
import {randomBytes} from 'node:crypto'
import {createOwnedProviderTls} from './e2e-real/provider-run-ownership.mjs'
import {selectProviderOutput} from './e2e-real/provider-run-output.mjs'
import {privateArtifacts} from './e2e-real/private-artifacts.mjs'

let providerTlsOwner

// Material exists before Chromium launch, allowing an issuer-only SPKI exception.
if (!process.env.LABBY_E2E_PROVIDER_TLS) {
  const tls = createOwnedProviderTls()
  providerTlsOwner = tls
  process.env.LABBY_E2E_PROVIDER_TLS = tls.root
  process.env.LABBY_E2E_PROVIDER_SPKI = tls.spki
}
if (!/^[A-Za-z0-9+/]{43}=$/.test(process.env.LABBY_E2E_PROVIDER_SPKI || '')) {
  throw new Error('The selected controlled issuer requires its exact SPKI pin')
}
process.env.LABBY_E2E_PROVIDER_EMAIL = 'owner@controlled.invalid'
process.env.LABBY_E2E_PROVIDER_PASSWORD ||= randomBytes(32).toString('hex')
process.env.LABBY_E2E_MEMBER_PASSWORD ||= randomBytes(32).toString('hex')
const providerOutputOwner = selectProviderOutput()

export default defineConfig({
  metadata: {providerTlsOwner, providerOutputOwner},
  testDir: './e2e-real',
  testMatch: ['provider-login.spec.mjs', 'provider-projects-isolation.spec.mjs', 'provider-access.spec.mjs'],
  workers: 1,
  retries: 0,
  maxFailures: 1,
  timeout: 90000,
  globalTimeout: 600000,
  outputDir: 'test-results/provider',
  globalTeardown: './e2e-real/provider-teardown.mjs',
  use: {
    baseURL: 'http://127.0.0.1:4328',
    headless: true,
    ...privateArtifacts(),
    launchOptions: {
      executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH,
      args: [`--ignore-certificate-errors-spki-list=${process.env.LABBY_E2E_PROVIDER_SPKI}`],
    },
  },
  webServer: {
    command: 'node e2e-real/provider-start.mjs',
    url: 'http://127.0.0.1:4328/sign-in',
    reuseExistingServer: false,
    timeout: 180000,
    gracefulShutdown: {signal: 'SIGTERM', timeout: 20000},
  },
})
