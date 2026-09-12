import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, copyFileSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';

test('unit discovery executes a fixture in a checkout with spaces and Unicode', () => {
  const root = mkdtempSync(join(tmpdir(), 'Labby Checkout é-'));
  try {
    mkdirSync(join(root, 'scripts'));
    mkdirSync(join(root, 'lib'));
    mkdirSync(join(root, 'node_modules/tsx'), { recursive: true });
    writeFileSync(join(root, 'node_modules/tsx/package.json'), JSON.stringify({ type: 'module', exports: { './cli': './cli.mjs' } }));
    writeFileSync(join(root, 'node_modules/tsx/cli.mjs'), `import { spawnSync } from 'node:child_process';
const result = spawnSync(process.execPath, process.argv.slice(2), { stdio: 'inherit' });
process.exit(result.status ?? 1);
`);
    copyFileSync(new URL('./run-unit-tests.mjs', import.meta.url), join(root, 'scripts/run-unit-tests.mjs'));
    writeFileSync(join(root, 'lib/fixture.test.ts'), "import test from 'node:test'; test('encoded checkout fixture', () => {});\n");
    const env = { ...process.env };
    delete env.NODE_TEST_CONTEXT;
    const result = spawnSync(process.execPath, [join(root, 'scripts/run-unit-tests.mjs')], {
      env, encoding: 'utf8',
    });
    assert.equal(result.status, 0, result.stdout + result.stderr);
    assert.match(result.stdout, /encoded checkout fixture/);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
