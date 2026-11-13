// Playwright 1.59.1 checks this before collecting automatic ARIA failure snapshots.
// Set it while loading config so every worker inherits the private-input policy.
export function privateArtifacts() {
  process.env.PLAYWRIGHT_NO_COPY_PROMPT = '1'
  return {trace: 'off', screenshot: 'off', video: 'off'}
}
