import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

const layoutSource = readFileSync(
  resolve(process.cwd(), 'app/(admin)/settings/layout.tsx'),
  'utf8',
)
const railSource = readFileSync(
  resolve(process.cwd(), 'components/settings/SettingsRail.tsx'),
  'utf8',
)
const sidebarSource = readFileSync(
  resolve(process.cwd(), 'components/app-sidebar.tsx'),
  'utf8',
)

test('settings layout owns a full-screen Unraid shell with primary navigation', () => {
  assert.match(layoutSource, /data-unraid-settings-shell/)
  assert.match(layoutSource, /className="[^"]*unraid-settings-shell/)
  assert.match(layoutSource, /href="\/"/)
  assert.match(layoutSource, /href="\/gateways"/)
  assert.match(layoutSource, /href="\/settings\/core\/"/)
  assert.doesNotMatch(layoutSource, /<AppHeader/)
})

test('settings layout carries the mock visual identity', () => {
  assert.match(layoutSource, /linear-gradient\(90deg,#e22828,#ff8c2f\)/)
  assert.match(layoutSource, /bg-\[#f2f2f2\]/)
  assert.match(layoutSource, /max-w-\[1440px\]/)
  assert.match(layoutSource, /<Power/)
  assert.match(layoutSource, /bg-\[#1c1b1b\]/)
})

test('settings navigation exposes Incus deployment preferences', () => {
  assert.match(railSource, /href: '\/settings\/deployment\/'/)
  assert.match(railSource, /label: 'Deployment'/)
})

test('the standard application sidebar is omitted behind the settings shell', () => {
  assert.match(sidebarSource, /if \(pathname\.startsWith\('\/settings'\)\) return null/)
})
