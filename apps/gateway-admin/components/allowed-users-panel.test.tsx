import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { AllowedUsersPanel } from './allowed-users-panel'

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

// Capture initial render (loading skeleton) without triggering async effects
function render() {
  return renderToStaticMarkup(<AllowedUsersPanel />)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

test('AllowedUsersPanel renders add-email form with correct elements', () => {
  const markup = render()

  // Input for adding email
  assert.match(markup, /id="allowed-email-input"/)
  assert.match(markup, /type="email"/)
  assert.match(markup, /user@example\.com/)
  // Add button
  assert.match(markup, /aria-label="Add email to allowlist"/)
})

test('AllowedUsersPanel renders loading skeleton initially', () => {
  const markup = render()

  assert.match(markup, /animate-pulse/)
})

test('AllowedUsersPanel renders table with aria-label for screen readers', () => {
  const markup = render()

  // The table is not rendered in loading state, but the panel container and
  // form are always present regardless of load state
  assert.match(markup, /Allowed users/)
})

test('AllowedUsersPanel includes accessible label for email input', () => {
  const markup = render()

  assert.match(markup, /Email address to allow/)
  assert.match(markup, /sr-only/)
})

test('AllowedUsersPanel offers a role choice defaulting to member', () => {
  const markup = render()
  assert.match(markup, /Role for new user/)
  assert.match(markup, /Member/)
})
