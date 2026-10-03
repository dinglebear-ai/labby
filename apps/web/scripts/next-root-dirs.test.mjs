import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, realpathSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname, resolve } from 'node:path';
import { createRequire } from 'node:module';
import { test } from 'node:test';
import { spawnSync } from 'node:child_process';

const require = createRequire(import.meta.url);
const pluginRequire = createRequire(require.resolve('@next/eslint-plugin-next/package.json'));
const { getRootDirs } = pluginRequire('./dist/utils/get-root-dirs.js');

function fixture(run) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'labby-next-roots-')));
  for (const path of ['apps/a', 'apps/b/nested', 'apps/.hidden']) mkdirSync(join(root, path), { recursive: true });
  writeFileSync(join(root, 'apps/file.txt'), 'not a project directory');
  try { run(root); } finally { rmSync(root, { recursive: true, force: true }); }
}

test('Next root discovery retains configured directory patterns and excludes files/dot directories', () => fixture((root) => {
  assert.deepEqual(getRootDirs({ cwd: root, settings: { next: { rootDir: `${root}/apps/*` } } }).map((path) => resolve(path)).sort(), [join(root, 'apps/a'), join(root, 'apps/b')]);
}));

test('Next root discovery retains brace alternatives, recursive patterns and mixed arrays', () => fixture((root) => {
  const roots = getRootDirs({ cwd: root, settings: { next: { rootDir: [`${root}/apps/{a,b}`, `${root}/apps/**/nested`, null] } } });
  assert.deepEqual(roots.map((path) => resolve(path)).sort(), [join(root, 'apps/a'), join(root, 'apps/b'), join(root, 'apps/b/nested')]);
}));

test('Next root discovery preserves default cwd, missing matches and path separator normalization', () => fixture((root) => {
  assert.deepEqual(getRootDirs({ cwd: root, settings: {} }), [root]);
  assert.deepEqual(getRootDirs({ cwd: root, settings: { next: { rootDir: `${root}/missing/*` } } }), []);
  assert.deepEqual(getRootDirs({ cwd: root, settings: { next: { rootDir: `${root}/apps/a`.replaceAll('/', '\\') } } }).map((path) => resolve(path)), [join(root, 'apps/a')]);
}));

test('Next lint dependency executes tinyglobby implementation without the vulnerable braces graph', () => {
  const resolved = pluginRequire.resolve('fast-glob/package.json');
  assert.equal(pluginRequire('fast-glob/package.json').name, 'tinyglobby');
  assert.equal(pluginRequire('fast-glob/package.json').version, '0.2.16');
  const globRequire = createRequire(join(dirname(resolved), 'package.json'));
  assert.throws(() => globRequire.resolve('braces'), { code: 'MODULE_NOT_FOUND' });
  assert.throws(() => globRequire.resolve('micromatch'), { code: 'MODULE_NOT_FOUND' });
});

test('Deep brace patterns cannot exhaust the lint root discovery stack', () => fixture((root) => {
  const result = spawnSync(process.execPath, ['-e', `const { getRootDirs } = require(${JSON.stringify(pluginRequire.resolve('./dist/utils/get-root-dirs.js'))}); const roots=getRootDirs({cwd:process.cwd(),settings:{next:{rootDir:'{'.repeat(4096)+'nonexistent'+'}'.repeat(4096)}}}); if(roots.length)process.exit(2);`], { cwd: root, timeout: 5000, encoding: 'utf8' });
  assert.equal(result.error, undefined);
  assert.equal(result.status, 0, result.stderr);
}));
