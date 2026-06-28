import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import type { ExtractReport } from '@/lib/api/extract-client'
import { SetupResults, setupResultDescription } from './setup-results'

test('setupResultDescription reflects fleet and targeted scans', () => {
  const fleet: ExtractReport = {
    target: { mode: 'fleet' },
    found: [],
    creds: [],
    warnings: [],
  }
  const targeted: ExtractReport = {
    target: { mode: 'targeted', uri: { host: 'node-b', path: '/mnt/appdata' } },
    found: [],
    creds: [],
    warnings: [],
  }

  assert.equal(setupResultDescription(fleet), 'Fleet scan across all actionable SSH config hosts.')
  assert.equal(
    setupResultDescription(targeted),
    'Targeted scan for node-b:/mnt/appdata.',
  )
})

test('setup results render secret presence without exposing the raw value', () => {
  const report: ExtractReport = {
    target: { mode: 'fleet' },
    found: ['overseerr'],
    creds: [
      {
        service: 'overseerr',
        url: 'http://100.64.0.20:5055/login',
        env_field: 'OVERSEERR_API_KEY',
        secret_present: true,
        source_host: 'node-b',
      },
    ],
    warnings: [],
  }

  const markup = renderToStaticMarkup(
    React.createElement(SetupResults, {
      report,
      sortedCreds: report.creds,
    }),
  )

  assert.match(markup, /Present/)
  assert.doesNotMatch(markup, /secret-key/)
  assert.match(markup, /OVERSEERR_API_KEY/)
})

test('setup results render warnings and empty states', () => {
  const warningReport: ExtractReport = {
    target: { mode: 'fleet' },
    found: [],
    creds: [],
    warnings: [
      {
        service: 'radarr',
        host: 'controller',
        message: 'endpoint probe failed',
      },
    ],
  }

  const emptyMarkup = renderToStaticMarkup(
    React.createElement(SetupResults, {
      report: null,
      sortedCreds: [],
    }),
  )
  const warningMarkup = renderToStaticMarkup(
    React.createElement(SetupResults, {
      report: warningReport,
      sortedCreds: [],
    }),
  )

  assert.match(emptyMarkup, /No scan results yet/)
  assert.match(warningMarkup, /Warnings/)
  assert.match(warningMarkup, /endpoint probe failed/)
})
