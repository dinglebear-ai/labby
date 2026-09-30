import test from 'node:test'
import assert from 'node:assert/strict'

import { openIsolatedOauthPopup } from './oauth-popup'

test('opens OAuth in an isolated browsing context and severs the opener', () => {
  const calls: Array<[string | URL | undefined, string | undefined, string | undefined]> = []
  const popup = {
    closed: false,
    opener: { location: 'trusted-admin' },
    location: { href: '' },
  }
  const browserWindow = {
    open: (url?: string | URL, target?: string, features?: string) => {
      calls.push([url, target, features])
      return popup
    },
  }

  const result = openIsolatedOauthPopup(browserWindow as Pick<Window, 'open'>)

  assert.equal(result, popup)
  assert.deepEqual(calls, [['about:blank', '_blank', undefined]])
  assert.equal(popup.opener, null)
})

test('reports a blocked popup without attempting opener mutation', () => {
  const browserWindow = { open: () => null }

  assert.equal(
    openIsolatedOauthPopup(browserWindow as Pick<Window, 'open'>),
    null,
  )
})

test('fails closed when the browser refuses to sever the opener', () => {
  let closed = false
  const popup = {
    set opener(_value: unknown) {
      throw new Error('opener is not writable')
    },
    close: () => {
      closed = true
    },
  }
  const browserWindow = { open: () => popup }

  assert.equal(
    openIsolatedOauthPopup(browserWindow as unknown as Pick<Window, 'open'>),
    null,
  )
  assert.equal(closed, true)
})
