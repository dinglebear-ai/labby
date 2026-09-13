import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'

test('console matches reference rail widths and centers global search without overlap', () => {
  const sidebar = readFileSync(new URL('./console-sidebar.tsx', import.meta.url), 'utf8')
  assert.match(sidebar, /SIDEBAR_WIDTH_EXPANDED = '224px'/)
  assert.match(sidebar, /SIDEBAR_WIDTH_COLLAPSED = '58px'/)
  const shellContext = readFileSync(new URL('./console-shell-context.tsx', import.meta.url), 'utf8')
  assert.match(shellContext, /useState\(true\)/)
  assert.match(shellContext, /saved === null \? true : saved === '1'/)
  const topbar = readFileSync(new URL('./console-topbar.tsx', import.meta.url), 'utf8')
  const search = topbar.slice(topbar.indexOf('data-searchbar="1"'), topbar.indexOf('data-topbar-rail="right"'))
  assert.match(search, /flex: '0 1 auto'/)
  // The pill is centred by two equal-basis rails, in flow: an absolutely
  // positioned pill let a wide status/action cluster slide underneath it.
  assert.doesNotMatch(search, /position: 'absolute'/)
  assert.doesNotMatch(search, /translateX\(-50%\)/)
  const left = topbar.indexOf('data-topbar-rail="left"')
  const pill = topbar.indexOf('data-searchbar="1"')
  const right = topbar.indexOf('data-topbar-rail="right"')
  assert.ok(left !== -1 && left < pill && pill < right, 'search pill sits between the two rails')
  for (const rail of ['left', 'right']) {
    const start = topbar.indexOf(`data-topbar-rail="${rail}"`)
    assert.match(topbar.slice(start, start + 200), /flex: '1 1 0'/)
  }
  const rightRail = topbar.slice(right)
  assert.match(rightRail, /ref={setActionSlot}/)
  assert.match(rightRail, /<ConsoleStatusStrip/)
  assert.match(search, /data-search-shortcut="1"/)
  assert.doesNotMatch(search, /Notifications|<Bell/)
  assert.match(search, /onClick={openPalette}/)
})
