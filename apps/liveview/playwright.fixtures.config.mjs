import {defineConfig} from '@playwright/test'
export default defineConfig({
  testDir: './e2e', outputDir: 'test-results/fixtures', workers: 1, retries: 0,
  use: {baseURL: 'http://127.0.0.1:4198', headless: true,
    launchOptions: {executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH}},
  webServer: {command: 'MIX_ENV=test mix run --no-start e2e/server.exs', url: 'http://127.0.0.1:4198/sign-in', reuseExistingServer: false, timeout: 60000}
})
