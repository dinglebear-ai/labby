import test from 'node:test'
import assert from 'node:assert/strict'
import { renderToStaticMarkup } from 'react-dom/server'

import { SettingsScalarSection } from './SettingsScalarSection'
import type { SettingsFieldSpec, SettingsState } from '@/lib/api/setup-client'

const fields: SettingsFieldSpec[] = [
  { key: 'LAB_LOG', label: 'Log filter', description: '', section: 'core', backend: 'env', control: 'text', risk: 'restart', write_policy: 'editable', apply_mode: 'restart', secret: false, required: false, env_override: null, min: null, max: null, options: [], example: null },
]

const state: SettingsState = {
  schema_version: 1,
  config_path: '/tmp/config.toml',
  env_path: '/tmp/.env',
  section: 'core',
  values: { LAB_LOG: 'lab=info' },
  sources: { LAB_LOG: { source: 'env', overridden_by_env: null } },
}

test('SettingsScalarSection renders reset and save controls', () => {
  const html = renderToStaticMarkup(
    <SettingsScalarSection title="Core" description="" section="core" state={state} fields={fields} onSaved={() => undefined} />,
  )
  assert.match(html, /Core/)
  assert.match(html, /Reset/)
  assert.match(html, /Save changes/)
})
