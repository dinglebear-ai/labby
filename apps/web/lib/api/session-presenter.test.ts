import test from 'node:test'
import assert from 'node:assert/strict'

import {
  sessionAvatarFallback,
  sessionPrimaryEmail,
} from '../auth/session-presenter.ts'

test('sessionPrimaryEmail prefers email over subject', () => {
  assert.equal(
    sessionPrimaryEmail({
      sub: 'user-123',
      email: 'person@example.com',
    }),
    'person@example.com',
  )
})

test('sessionPrimaryEmail falls back to subject when email is absent', () => {
  assert.equal(
    sessionPrimaryEmail({
      sub: 'user-123',
      email: null,
    }),
    'user-123',
  )
})

test('sessionAvatarFallback uses the first email letter', () => {
  assert.equal(
    sessionAvatarFallback({
      sub: 'user-123',
      email: 'person@example.com',
    }),
    'P',
  )
})

test('sessionAvatarFallback falls back to subject initial', () => {
  assert.equal(
    sessionAvatarFallback({
      sub: 'zebra-123',
      email: undefined,
    }),
    'Z',
  )
})
