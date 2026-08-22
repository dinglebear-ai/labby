import test from 'node:test'
import assert from 'node:assert/strict'

import { loadoutSaveEnabled } from './loadout-form-dialog.tsx'

test('loadout save remains disabled until gateway options load successfully', () => {
  assert.equal(loadoutSaveEnabled(false, true, null, 'operations', 4, false), false)
  assert.equal(loadoutSaveEnabled(false, false, 'Gateway options failed', 'operations', 4, false), false)
  assert.equal(loadoutSaveEnabled(false, false, null, 'operations', 4, false), true)
})
