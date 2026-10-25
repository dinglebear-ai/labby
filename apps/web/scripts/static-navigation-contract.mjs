import assert from 'node:assert/strict'

/** Check the root record emitted by the pinned Next.js static RSC export. */
export function verifyStaticNavigationBuildId({ buildId, tree, html }) {
  assert.ok(typeof buildId === 'string' && buildId.length > 0, 'Next must emit a non-empty build ID')
  const roots = tree.split(/\r?\n/).filter(line => line.startsWith('0:'))
  assert.equal(roots.length, 1, 'the exported navigation payload must have exactly one root record')
  const navigation = JSON.parse(roots[0].slice(2))
  assert.equal(
    navigation?.b,
    buildId,
    'the exported navigation payload must carry the current build ID',
  )
  assert.doesNotMatch(html, /data-dpl-id|[?&]dpl=/, 'static exports must not use server-coordinated deployment IDs')
}
