import test from 'node:test'
import assert from 'node:assert/strict'
import {
  builderInputSpecs,
  inputSpecFromSchema,
  inputSpecsFrontmatter,
} from './builder-inputs'
test('guided edit preserves complete existing input contracts without requiring example values', () => {
  const existing = {
    required: {
      ty: 'number' as const,
      required: true,
      nullable: false,
      description: 'Current ratio',
    },
    optional: { ty: 'boolean' as const, required: false, default: false },
    nullable: { ty: 'json' as const, nullable: true, default: null },
  }
  const result = builderInputSpecs(existing, {
    required: 1,
    optional: true,
    newInput: 'value',
  })
  for (const name of Object.keys(existing))
    assert.deepEqual(result[name], existing[name as keyof typeof existing])
  assert.match(
    inputSpecsFrontmatter(result).join('\n'),
    /required:\n    type: number\n    required: true\n    nullable: false\n    description: Current ratio/,
  )
  assert.equal(
    Object.prototype.hasOwnProperty.call(result.required, 'default'),
    false,
  )
})
test('new required inputs preserve schema type without manufactured defaults and omit nested credentials', () => {
  assert.deepEqual(
    inputSpecFromSchema({ type: 'number', description: 'Ratio' }),
    { ty: 'number', required: true, description: 'Ratio' },
  )
  assert.deepEqual(
    builderInputSpecs({}, { settings: { password: 'secret' }, host: 'node' }),
    {
      settings: { ty: 'json', required: true },
      host: { ty: 'string', default: 'node', required: false },
    },
  )
})
