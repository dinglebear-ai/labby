import test from 'node:test'
import assert from 'node:assert/strict'

import { normalizeTeamInvitationToken, teamInvitationHref, teamInvitationTokenFromHash } from './team-invitation.ts'

test('invitation tokens normalize to exact lowercase 32-byte hex values', () => {
  const upper = 'AB'.repeat(32)
  assert.equal(normalizeTeamInvitationToken('  ' + upper + '  '), upper.toLowerCase())
  assert.equal(normalizeTeamInvitationToken('ab'), undefined)
  assert.equal(normalizeTeamInvitationToken('zz'.repeat(32)), undefined)
})

test('invitation parser reads only the URL fragment', () => {
  const token = '12'.repeat(32)
  assert.equal(teamInvitationTokenFromHash('#invite=' + token), token)
  assert.equal(teamInvitationTokenFromHash('#other=value&invite=' + token), token)
  assert.equal(teamInvitationTokenFromHash('?invite=' + token), undefined)
  assert.equal(teamInvitationTokenFromHash('#invite=short'), undefined)
})

test('generated invitation links keep the secret strictly after the fragment marker', () => {
  const token = 'cd'.repeat(32)
  const href = teamInvitationHref('https://team.example/', token)
  assert.equal(href, 'https://team.example/#invite=' + token)
  const [requestUrl, fragment] = href.split('#', 2)
  assert.equal(requestUrl, 'https://team.example/')
  assert.equal(requestUrl.includes(token), false)
  assert.equal(fragment, 'invite=' + token)
})

test('generated invitation links reject malformed tokens instead of leaking arbitrary text', () => {
  assert.throws(() => teamInvitationHref('https://team.example', 'not-a-token'))
})
