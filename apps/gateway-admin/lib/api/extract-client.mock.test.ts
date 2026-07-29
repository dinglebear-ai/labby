import assert from 'node:assert/strict'
import test from 'node:test'

process.env.NEXT_PUBLIC_MOCK_DATA = 'true'

const { extractApi } = await import('./extract-client.ts')

test('mock extract report marks missing Apprise credentials consistently', async () => {
  const report = await extractApi.scan()
  const apprise = report.creds.find((credential) => credential.service === 'apprise')

  assert.ok(apprise)
  assert.equal(apprise.secret_present, false)
  assert.ok(
    report.warnings.some(
      (warning) =>
        warning.service === 'apprise' && warning.message.includes('no API token'),
    ),
  )
})
