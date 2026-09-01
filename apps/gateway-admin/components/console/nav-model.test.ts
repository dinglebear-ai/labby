import test from 'node:test'
import assert from 'node:assert/strict'

import {
  consoleNavItems,
  consoleNavSections,
  consoleNavSectionsForScope,
  isNavItemActive,
} from './nav-model'

// bead lab-vl9q6
test('the first nine nav items expose truthful single-digit accelerators', () => {
  // console-sidebar.tsx's ⌘/Ctrl+N handler jumps to
  // `consoleNavSections.flatMap(section => section.items)[N - 1]` — the
  // displayed accelerator must match that exact position or the hint lies
  // about what pressing it does. This previously drifted: Loadouts was
  // inserted without renumbering what followed it, so Tools/Loadouts both
  // showed ⌘3 and Usage/Traces both showed ⌘6, none matching the real
  // handler once Skills, Usage, and Traces were counted in.
  consoleNavItems.forEach((item, index) => {
    const expected = index < 9 ? `⌘${index + 1}` : ''
    assert.equal(item.kbd, expected, `${item.id} should expose only a usable accelerator`)
    if (expected) assert.ok(item.tooltip.includes(expected))
  })
})

test('every nav item kbd accelerator is unique', () => {
  const seen = new Set<string>()
  for (const item of consoleNavItems) {
    if (!item.kbd) continue
    assert.ok(!seen.has(item.kbd), `duplicate accelerator ${item.kbd} on ${item.id}`)
    seen.add(item.kbd)
  }
})

test('team navigation only appears in the team workspace scope', () => {
  const personal = consoleNavSectionsForScope('personal')
  const team = consoleNavSectionsForScope('team')
  assert.deepEqual(personal.map((section) => section.id), [
    'Control Plane',
    'Depot',
    'Workspace',
  ])
  assert.deepEqual(team.map((section) => section.id), [
    'Control Plane',
    'Depot',
    'Workspace',
    'Team',
  ])
  assert.equal(team.flatMap((section) => section.items).find((item) => item.id === 'Library')?.href, '/team/library')
  assert.equal(team.flatMap((section) => section.items).find((item) => item.id === 'Stash')?.href, '/team/stash')
})

test('scoped navigation uses the mock accelerator contract', () => {
  const byId = new Map(
    consoleNavSectionsForScope('personal')
      .flatMap((section) => section.items)
      .map((item) => [item.id, item.kbd]),
  )
  assert.deepEqual(Object.fromEntries(byId), {
    Overview: '⌘4',
    Gateway: '⌘5',
    Instance: '',
    Logs: '⌘6',
    Discovery: '⌘1',
    Create: '⌘2',
    Library: '⌘3',
    Agents: '',
    WorkspaceTasks: '',
    Stash: '',
    Containers: '',
  })
})

test('scoped navigation exposes visibly attributable fixture context', () => {
  for (const scope of ['personal', 'team'] as const) {
    for (const item of consoleNavSectionsForScope(scope).flatMap((section) => section.items)) {
      assert.ok(item.contextLine, `${scope}:${item.id} should provide active-route context`)
      assert.equal(item.contextIsMock, true, `${scope}:${item.id} fixture context must be marked mock`)
    }
  }
})

test('consoleNavItems is the flattened consoleNavSections in section order', () => {
  const flat = consoleNavSections.flatMap((section) => section.items)
  assert.deepEqual(
    consoleNavItems.map((item) => item.id),
    flat.map((item) => item.id),
  )
})

test('team subroutes activate only their specific destination', () => {
  const teamItems = consoleNavSectionsForScope('team').flatMap((section) => section.items)
  for (const pathname of ['/team', '/team/library', '/team/projects', '/team/activity']) {
    const active = teamItems.filter((item) => isNavItemActive(item.href, pathname))
    assert.equal(active.length, 1, `${pathname} should have exactly one active item`)
    assert.equal(active[0]?.href, pathname)
  }
})
