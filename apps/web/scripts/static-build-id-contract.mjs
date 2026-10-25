import assert from 'node:assert/strict'

export function assertStaticNavigationBuildId(buildId, tree, html) {
  assert.equal(typeof buildId, 'string', 'Next must emit a string build ID')
  assert.ok(buildId.trim().length > 0, 'Next must emit a non-empty build ID')
  const roots = tree.split('\n').filter(line => line.startsWith('0:'))
  assert.equal(roots.length, 1, 'the navigation payload must contain exactly one root Flight record')
  let root
  try { root = JSON.parse(roots[0].slice(2)) }
  catch { assert.fail('the navigation payload root Flight record must be JSON') }
  assert.ok(root !== null && typeof root === 'object' && !Array.isArray(root), 'the navigation payload root must be an object')
  // Next 16.4's serializer and segment-cache reader use b for the build ID.
  assert.equal(typeof root.b, 'string', 'the navigation payload root must carry a string build ID')
  assert.equal(root.b, buildId, 'the exported Loadouts navigation payload must carry the current build ID')
  assert.doesNotMatch(html, /data-dpl-id|[?&]dpl=/, 'static exports must not use server-coordinated deployment IDs')
}
