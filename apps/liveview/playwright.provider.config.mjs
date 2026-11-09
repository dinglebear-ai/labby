import {defineConfig} from '@playwright/test'
import {randomBytes} from 'node:crypto'
import {prepareProviderTls} from './e2e-real/provider-issuer.mjs'

// Material exists before Chromium launch, allowing an issuer-only SPKI exception.
if (!process.env.LABBY_E2E_PROVIDER_TLS) {
  const tls = prepareProviderTls()
  process.env.LABBY_E2E_PROVIDER_TLS = tls.root
  process.env.LABBY_E2E_PROVIDER_SPKI = tls.spki
  process.env.LABBY_E2E_PROVIDER_TLS_OWNED = 'true'
}
if (!/^[A-Za-z0-9+/]{43}=$/.test(process.env.LABBY_E2E_PROVIDER_SPKI || '')) {
  throw new Error('The selected controlled issuer requires its exact SPKI pin')
}
process.env.LABBY_E2E_PROVIDER_EMAIL = 'owner@controlled.invalid'
process.env.LABBY_E2E_PROVIDER_PASSWORD ||= randomBytes(32).toString('hex')
process.env.LABBY_E2E_MEMBER_PASSWORD ||= randomBytes(32).toString('hex')

export default defineConfig({
  testDir: './e2e-real',
  testMatch: ['provider-login.spec.mjs', 'provider-projects-isolation.spec.mjs'],
  workers: 1,
  retries: 0,
  maxFailures: 1,
  timeout: 90000,
  globalTimeout: 300000,
  outputDir: 'test-results/provider',
  globalTeardown: './e2e-real/provider-teardown.mjs',
  use: {
    baseURL: 'http://127.0.0.1:4328',
    headless: true,
    trace: 'off',
    screenshot: 'off',
    video: 'off',
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
