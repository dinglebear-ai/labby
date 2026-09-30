import { existsSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright'
import { ownedBrowserLaunchOptions } from './live-backend-harness.ts'

async function main(): Promise<void> {
  const marker = process.env.LABBY_E2E_BROWSER_FIXTURE_MARKER
  if (!marker) throw new Error('owned browser fixture marker required')
  const executable = fileURLToPath(new URL('./detached-browser.fixture.mjs', import.meta.url))
  // Exercise Playwright's real detached launcher, but do not require a downloaded
  // Chromium build merely to prove process-tree containment.
  void chromium.launch({ ...await ownedBrowserLaunchOptions(executable), timeout: 0 }).catch(() => {})
  while (!existsSync(marker)) await new Promise((resolve) => setTimeout(resolve, 10))
  process.on('SIGTERM', () => {})
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0)
}

void main().catch((error) => { console.error(error); process.exitCode = 1 })
