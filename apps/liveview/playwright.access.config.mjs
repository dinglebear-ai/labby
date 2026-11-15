import {defineConfig} from '@playwright/test'
import {privateArtifacts} from './e2e-real/private-artifacts.mjs'
export default defineConfig({
  testDir: './e2e', testMatch: 'access.spec.mjs', outputDir: 'test-results/access',
  workers: 1, retries: 0, timeout: 90000,
  use: {baseURL: 'http://127.0.0.1:4198', headless: true, ...privateArtifacts(),
    launchOptions: {executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH}},
  webServer: {command: 'MIX_ENV=test mix run --no-start e2e/access-server.exs', url: 'http://127.0.0.1:4198/sign-in', reuseExistingServer: false, timeout: 60000}
})
