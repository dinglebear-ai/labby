import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'

const source = readFileSync(new URL('./page.tsx', import.meta.url), 'utf8')

test('Activity reference composition places explanation and complete-window status in the hero', () => {
  const explanation = 'Every retained upstream call in the selected slice.'
  assert.equal(source.split(explanation).length - 1, 1)
  assert.match(source, /breadcrumbs=\{\[\{ label: 'Activity' \}\]\}/)
  assert.match(source, /complete-window analytics/)
  assert.match(source, /style=\{\{ gap: 14 \}\}/)
  assert.match(source, /title="Usage Explorer"\s+description="Every retained upstream call/)
  assert.match(source, /aria-label="Search upstream calls"/)
  assert.match(source, /aria-label="Server"/)
  assert.match(source, /aria-label="Outcome"/)
})

test('Activity dense desktop table retains mobile cards, Surface, and a call inspector', () => {
  assert.match(source, /aurora-scrollbar hidden overflow-x-auto md:block/)
  assert.match(source, /Table data-density="default"/)
  assert.match(source, /bodyStyle=\{\{ padding: 0, gap: 0 \}\}/)
  assert.match(source, /data-activity-filters="1"/)
  assert.match(source, /const activityGridColumns = \[/)
  assert.match(source, /showSurfaces \? '70px' : null/)
  assert.match(source, /gridTemplateColumns: activityGridColumns/)
  assert.match(source, /height: 28,/)
  assert.match(source, /height: 59,/)
  assert.match(source, /<UsageCallCards calls=\{data\?\.calls\}/)
  assert.match(source, /\{showSurfaces \? <TableHead className="w-\[70px\]">Surface<\/TableHead> : null\}/)
  assert.match(source, /\{showSurfaces \? <TableCell><SurfaceTag surface=\{call\.surface\} \/><\/TableCell> : null\}/)
  assert.match(source, /aria-label=\{`Inspect call \$\{\[call\.tool, call\.action\]\.filter\(Boolean\)\.join\('\.'\)\}`\}/)
  assert.match(source, /<UsageCallDetail/)
  assert.match(source, /setSelectedCall\(call\)/)
  assert.match(source, /showTokens \? <TableHead/)
  assert.match(source, /call\.agent_label === 'unattributed' \? 'Not attributed'/)
})

test('Activity presentation keeps URL-scoped filters and bounded cursor pagination', () => {
  assert.match(source, /const PAGE_SIZE = 50/)
  assert.match(source, /limit: PAGE_SIZE,\s+cursor,/)
  assert.match(source, /router\.replace\(query \? `\/usage\/\?\$\{query\}` : '\/usage\/', \{ scroll: false \}\)/)
  assert.match(source, /disabled=\{!data\?\.next_cursor\}/)
})
