import { readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { spawnSync } from "node:child_process";

const repoRoot = new URL("..", import.meta.url).pathname;

const testSpecs = [
  { dir: "lib", recursive: false, suffix: ".test.ts" },
  { dir: "lib/code-mode-app", recursive: false, suffix: ".test.ts" },
  { dir: "lib/server", recursive: true, suffix: ".test.ts" },
  { dir: "lib/api", recursive: true, suffix: ".test.ts" },
  { dir: "lib/dashboard", recursive: true, suffix: ".test.ts" },
  { dir: "lib/fs", recursive: true, suffix: ".test.ts" },
  { dir: "lib/dev", recursive: true, suffix: ".test.ts" },
  { dir: "lib/setup", recursive: false, suffix: ".test.ts" },
  { dir: "components", recursive: true, suffix: ".test.tsx" },
];

function collectTests(dir, { recursive, suffix }) {
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
    const stats = statSync(path);
    if (stats.isDirectory()) {
      return recursive ? collectTests(relative(repoRoot, path), { recursive, suffix }) : [];
    }
    return entry.endsWith(suffix) ? [relative(repoRoot, path)] : [];
  });
}

const tests = [...new Set(testSpecs.flatMap((spec) => collectTests(spec.dir, spec)))].sort();

if (tests.length === 0) {
  console.error("No unit tests found.");
  process.exit(1);
}

const command = process.platform === "win32" ? "tsx.cmd" : "tsx";
const result = spawnSync(command, ["--test", ...tests], {
  cwd: repoRoot,
  stdio: "inherit",
});

if (result.error) {
  throw result.error;
}

process.exit(result.status ?? 1);
