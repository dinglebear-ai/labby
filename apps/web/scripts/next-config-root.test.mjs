import test from 'node:test'
import assert from 'node:assert/strict'
import path from 'node:path'
import { resolveTurbopackRoot } from '../next.config.mjs'

const projects = path.join(path.parse(process.cwd()).root, 'projects')
const app = path.join(projects, 'labby', 'apps', 'web')

test('ordinary dependency installs keep the app root', () => {
  assert.equal(resolveTurbopackRoot(app, () => path.join(app, 'node_modules')), app)
})

test('linked worktree dependencies and source remain in the configured root', () => {
  assert.equal(resolveTurbopackRoot(path.join(projects, 'worktrees', 'labby', 'apps', 'web'), () => path.join(app, 'node_modules')), projects)
})

test('missing dependencies preserve normal pre-install configuration, while permission failures surface', () => {
  assert.equal(resolveTurbopackRoot(app, () => { throw Object.assign(new Error('missing'), { code: 'ENOENT' }) }), app)
  assert.throws(() => resolveTurbopackRoot(app, () => { throw Object.assign(new Error('denied'), { code: 'EACCES' }) }), /Cannot resolve/)
})

test('unrelated filesystem branches fail without widening the root to the filesystem', () => {
  assert.throws(() => resolveTurbopackRoot(path.join(path.parse(projects).root, 'source', 'apps', 'web'), () => path.join(path.parse(projects).root, 'dependencies', 'node_modules')), /filesystem-wide/)
})
