import {defineConfig} from '@playwright/test'
import {randomBytes} from 'node:crypto'
import {mkdtempSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {join} from 'node:path'

// A fresh synthetic credential for this owned daemon, never an operator token.
process.env.LABBY_E2E_CREDENTIAL ||= randomBytes(32).toString('hex')
process.env.LABBY_E2E_CONTROL ||= randomBytes(32).toString('hex')
process.env.LABBY_E2E_RESTRICTED_ROOT ||= mkdtempSync(join(tmpdir(), 'labby-real-restricted-'))
export default defineConfig({
  testDir: './e2e-real', testMatch: '**/*.spec.mjs', workers: 1, retries: 0, timeout: 60000,
  outputDir: 'test-results/real', maxFailures: 3, globalTimeout: 600000,
  use: {
    baseURL: 'http://127.0.0.1:4298', headless: true,
    launchOptions: {executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH},
    trace: 'off', // Sign-in contains an ephemeral credential; never record it.
    screenshot: 'only-on-failure',
  },
  webServer: {
    command: 'node e2e-real/start.mjs',
    url: 'http://127.0.0.1:4298/sign-in', reuseExistingServer: false,
    timeout: 180000,
    gracefulShutdown: {signal: 'SIGTERM', timeout: 20000},
    env: {LABBY_E2E_CREDENTIAL: process.env.LABBY_E2E_CREDENTIAL, LABBY_E2E_CONTROL: process.env.LABBY_E2E_CONTROL, LABBY_E2E_RESTRICTED_ROOT: process.env.LABBY_E2E_RESTRICTED_ROOT},
  },
})
