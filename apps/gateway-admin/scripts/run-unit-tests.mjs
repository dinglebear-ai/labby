import { readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { spawnSync } from "node:child_process";

const repoRoot = new URL("..", import.meta.url).pathname;

// Roots walked recursively for unit tests. This used to be a hand-maintained
// list of ~10 specific directories with per-entry suffixes, which silently
// dropped any test file added outside it: `components/**/*.test.ts` (four
// files) and all of `lib/hooks`, `lib/editor`, and `lib/settings` (six files)
// had never run in CI. A stale assertion can sit green in an unrun file
// indefinitely, so collection is now "walk everything and exclude
// deliberately" rather than "opt each directory in".
const TEST_ROOTS = ["app", "components", "hooks", "lib"];
const TEST_SUFFIXES = [".test.ts", ".test.tsx"];

// Names skipped at any depth — these never contain first-party tests.
const EXCLUDED_DIR_NAMES = new Set(["node_modules", ".next", "out"]);

// Skipped by exact path, not by name. `lib/browser/**` is owned by the separate
// `pnpm test:browser` command (see package.json, pinned by
// lib/tooling-contract.test.ts) because those tests need
// `--experimental-strip-types`. Matching the bare name "browser" instead would
// silently drop any future `components/**/browser/` directory — reintroducing
// exactly the never-run-tests hole this walker exists to close.
const EXCLUDED_PATHS = new Set(["lib/browser"]);

const isTestFile = (name) => TEST_SUFFIXES.some((suffix) => name.endsWith(suffix));

function collectTests(dir) {
  const root = join(repoRoot, dir);
  let entries;
  try {
    entries = readdirSync(root);
  } catch (error) {
    if (error?.code === "ENOENT") {
      return [];
    }
    throw error;
  }

  return entries.flatMap((entry) => {
    const path = join(root, entry);
    const relativePath = relative(repoRoot, path);
    const stats = statSync(path);
    if (stats.isDirectory()) {
      return EXCLUDED_DIR_NAMES.has(entry) || EXCLUDED_PATHS.has(relativePath)
        ? []
        : collectTests(relativePath);
    }
    return isTestFile(entry) ? [relativePath] : [];
  });
}

const tests = [...new Set(TEST_ROOTS.flatMap((root) => collectTests(root)))].sort();

if (tests.length === 0) {
  console.error("No unit tests found.");
  process.exit(1);
}

// Fail loudly if a test file exists outside the walked roots, so the next
// misplaced file is a build error rather than another silently-skipped suite.
// Covers both a stray directory and a stray file sitting at the package root —
// the latter is the likelier accident, and `isDirectory()` alone would skip it.
const strayRoots = readdirSync(repoRoot).filter((entry) => {
  if (TEST_ROOTS.includes(entry) || EXCLUDED_DIR_NAMES.has(entry)) return false;
  const path = join(repoRoot, entry);
  return statSync(path).isDirectory() ? collectTests(entry).length > 0 : isTestFile(entry);
});
if (strayRoots.length > 0) {
  console.error(
    `Found test files outside the walked roots (${TEST_ROOTS.join(", ")}): ` +
      `${strayRoots.join(", ")}. Add the directory to TEST_ROOTS or move the tests.`,
  );
  process.exit(1);
}

console.log(`running ${tests.length} unit test file(s)`);

const command = process.platform === "win32" ? "tsx.cmd" : "tsx";
const result = spawnSync(command, ["--test", ...tests], {
  cwd: repoRoot,
  stdio: "inherit",
});

if (result.error) {
  throw result.error;
}

process.exit(result.status ?? 1);
