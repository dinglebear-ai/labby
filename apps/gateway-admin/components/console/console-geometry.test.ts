import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'

test('console matches reference rail widths and keeps global search in flex flow', () => {
  const sidebar = readFileSync(new URL('./console-sidebar.tsx', import.meta.url), 'utf8')
  assert.match(sidebar, /SIDEBAR_WIDTH_EXPANDED = '224px'/)
  assert.match(sidebar, /SIDEBAR_WIDTH_COLLAPSED = '58px'/)
  const shellContext = readFileSync(new URL('./console-shell-context.tsx', import.meta.url), 'utf8')
  assert.match(shellContext, /useState\(true\)/)
  assert.match(shellContext, /saved === null \? true : saved === '1'/)
  const topbar = readFileSync(new URL('./console-topbar.tsx', import.meta.url), 'utf8')
  const search = topbar.slice(topbar.indexOf('data-searchbar="1"'), topbar.indexOf('ref={setActionSlot}'))
  assert.match(search, /flex: '0 1 auto'/)
  assert.doesNotMatch(search, /position: 'absolute'|translateX/)
  assert.match(search, /data-search-shortcut="1"/)
  assert.doesNotMatch(search, /Notifications|<Bell/)
  assert.match(search, /onClick={openPalette}/)
})
