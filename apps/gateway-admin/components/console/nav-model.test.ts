import test from 'node:test'
import assert from 'node:assert/strict'
import { readdirSync, statSync } from 'node:fs'
import { join, relative, sep } from 'node:path'

import { allowsProjectBoundSessionFallback, capabilityAwareNavSections, capabilityForPath, consoleNavItems, consoleNavSections } from './nav-model'

const ADMIN_APP_ROOT = new URL('../../app/(admin)/', import.meta.url).pathname

/** Every `page.tsx` under `app/(admin)`, converted to the pathname `usePathname` reports for it. */
function shippedAdminRoutes(dir = ADMIN_APP_ROOT): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry)
    if (statSync(path).isDirectory()) return shippedAdminRoutes(path)
    if (entry !== 'page.tsx') return []
    const segments = relative(ADMIN_APP_ROOT, dir).split(sep).filter(Boolean)
      // A dynamic segment stands in for any concrete value the router accepts.
      .map((segment) => (segment.startsWith('[') && segment.endsWith(']') ? 'example' : segment))
    return [`/${segments.join('/')}`]
  }).sort()
}

/** Routes whose pages configure the installation itself; they must never be open to every principal. */
const ADMIN_ROUTE_PREFIXES = ['/administration', '/browsers', '/design-system', '/docs', '/gateway', '/logs', '/settings']

// bead lab-vl9q6
test('every nav item kbd accelerator matches its position in the flattened list', () => {
  // console-sidebar.tsx's ⌘/Ctrl+N handler jumps to
  // `consoleNavSections.flatMap(section => section.items)[N - 1]` — the
  // displayed accelerator must match that exact position or the hint lies
  // about what pressing it does. This previously drifted: Loadouts was
  // inserted without renumbering what followed it, so Tools/Loadouts both
  // showed ⌘3 and Usage/Traces both showed ⌘6, none matching the real
  // handler once Skills, Usage, and Traces were counted in.
  consoleNavItems.forEach((item, index) => {
    assert.equal(item.kbd, `⌘${index + 1}`, `${item.id} should show ⌘${index + 1}`)
    assert.ok(
      item.tooltip.includes(item.kbd),
      `${item.id} tooltip should reference its own accelerator`,
    )
  })
})

test('every nav item kbd accelerator is unique', () => {
  const seen = new Set<string>()
  for (const item of consoleNavItems) {
    assert.ok(!seen.has(item.kbd), `duplicate accelerator ${item.kbd} on ${item.id}`)
    seen.add(item.kbd)
  }
})

test('consoleNavItems is the flattened consoleNavSections in section order', () => {
  const flat = consoleNavSections.flatMap((section) => section.items)
  assert.deepEqual(
    consoleNavItems.map((item) => item.id),
    flat.map((item) => item.id),
  )
})
test('Depot and Workspace navigation match the unified product information architecture', () => {
  const depot = consoleNavSections.find((section) => section.id === 'Depot')
  const workspace = consoleNavSections.find((section) => section.id === 'Workspace')

  assert.deepEqual(depot?.items.map((item) => item.label), ['Discover', 'Create', 'Library', 'Administration'])
  assert.deepEqual(workspace?.items.map((item) => item.label), [
    'Agents',
    'Tasks',
    'Dev Containers',
  ])
  assert.equal(consoleNavItems.some((item) => item.href === '/stash'), false)
  assert.equal(consoleNavItems.some((item) => item.label === 'Loadouts'), false)
  assert.equal(consoleNavItems.some((item) => item.label === 'Snippets'), false)
})

test('browser bridge is a real control-plane destination', () => {
  const browsers = consoleNavItems.find((item) => item.id === 'Browsers')
  assert.ok(browsers)
  assert.equal(browsers.href, '/browsers')
})

test('one filtered model removes denied links and shortcut targets together', () => {
  const member = capabilityAwareNavSections(['scope.read', 'scope.operate'])
  const items = member.flatMap((section) => section.items)
  const ids = items.map((item) => item.id)
  assert.ok(ids.includes('Agents'))
  assert.ok(ids.includes('Library'))
  assert.ok(!ids.includes('Labby'))
  assert.ok(!ids.includes('Logs'))
  assert.ok(!ids.includes('Create'))
  items.forEach((item, index) => assert.equal(item.kbd, `⌘${index + 1}`))
})

test('direct route manifest fails closed for unknown routes and gates known ones', () => {
  assert.equal(capabilityForPath('/settings/core'), 'platform.manage')
  assert.equal(capabilityForPath('/dev-containers'), 'scope.operate')
  assert.equal(capabilityForPath('/depot'), 'scope.read')
  assert.equal(capabilityForPath('/administration'), 'platform.manage')
  assert.equal(capabilityForPath('/projects'), 'scope.read')
  assert.equal(capabilityForPath('/stash'), 'scope.read')
  assert.equal(capabilityForPath('/gateway'), 'platform.manage')
  assert.equal(capabilityForPath('/not-a-product-route'), undefined)
})

test('only Skills admits a project-bound session without a durable authority projection', () => {
  assert.equal(allowsProjectBoundSessionFallback('/skills'), true)
  assert.equal(allowsProjectBoundSessionFallback('/skills/example'), true)
  assert.equal(allowsProjectBoundSessionFallback('/library'), false)
  assert.equal(allowsProjectBoundSessionFallback('/depot'), false)
})

test('every shipped app/(admin) route resolves to a capability instead of locking itself out', () => {
  const routes = shippedAdminRoutes()
  assert.ok(routes.includes('/projects') && routes.includes('/stash') && routes.includes('/settings/services/example'), `route enumeration must read the real app tree, saw: ${routes.join(', ')}`)
  const unresolved = routes.filter((route) => capabilityForPath(route) === undefined)
  assert.deepEqual(unresolved, [], 'a shipped route with no manifest entry is unreachable for every principal')
  const open = routes.filter((route) => capabilityForPath(route) === null)
  assert.deepEqual(open, [], 'every shipped route requires a server-projected capability')
  for (const route of routes) {
    if (ADMIN_ROUTE_PREFIXES.some((prefix) => route === prefix || route.startsWith(`${prefix}/`))) {
      assert.equal(capabilityForPath(route), 'platform.manage', `${route} configures the installation and must require platform.manage`)
    }
  }
})

test('workspace routes stay reachable for a plain member and admin routes do not', () => {
  const member = ['scope.read', 'scope.operate']
  const reachable = shippedAdminRoutes().filter((route) => { const required = capabilityForPath(route); return required !== null && required !== undefined && member.includes(required) })
  for (const route of ['/', '/projects', '/stash', '/depot', '/library', '/agents', '/tasks', '/dev-containers']) assert.ok(reachable.includes(route), `${route} must be reachable for a member`)
  for (const route of ['/administration', '/settings', '/logs', '/browsers']) assert.ok(!reachable.includes(route), `${route} must not be reachable for a member`)
})

test('control-plane navigation excludes the redundant Labby settings shortcut', () => {
  const controlPlane = consoleNavSections.find((section) => section.id === 'Control Plane')
  assert.deepEqual(controlPlane?.items.map((item) => item.id), ['Overview', 'Gateway', 'Browsers'])
  assert.equal(consoleNavItems.some((item) => item.id === 'Labby'), false)
  assert.equal(consoleNavItems.some((item) => item.href === '/settings/surfaces'), false)
})
