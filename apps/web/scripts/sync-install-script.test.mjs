import { copyFile, mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { spawnSync } from "node:child_process";
import assert from "node:assert/strict";

import { syncInstallScript } from "./sync-install-script.mjs";

test("syncInstallScript copies the repo installer into public assets", async () => {
  const root = await mkdtemp(join(tmpdir(), "labby-install-sync-"));
  const repoRoot = join(root, "repo");
  const appRoot = join(repoRoot, "apps/web");
  const installer = "#!/usr/bin/env sh\nset -eu\necho labby\n";

  await mkdir(join(repoRoot, "scripts"), { recursive: true });
  await writeFile(join(repoRoot, "scripts/install.sh"), installer, {
    mode: 0o755,
  });

  const { target, rootTarget } = await syncInstallScript({ appRoot, repoRoot });

  assert.equal(await readFile(target, "utf8"), installer);
  assert.equal(await readFile(rootTarget, "utf8"), installer);
  assert.equal((await stat(target)).mode & 0o111, 0o111);
});

test("checked-in public installer matches the repo installer", async () => {
  const appRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const repoRoot = resolve(appRoot, "../..");
  const source = await readFile(join(repoRoot, "scripts/install.sh"), "utf8");
  const publicCopy = await readFile(join(appRoot, "public/install.sh"), "utf8");
  const rootCopy = await readFile(join(repoRoot, "install.sh"), "utf8");

  assert.equal(publicCopy, source);
  assert.equal(rootCopy, source);
});


test("installer sync CLI runs from paths containing spaces and Unicode", async () => {
  const root = await mkdtemp(join(tmpdir(), "Labby Install é-"));
  try {
    const appRoot = join(root, "apps/web");
    await mkdir(join(appRoot, "scripts"), { recursive: true });
    await mkdir(join(root, "scripts"));
    const installer = "#!/usr/bin/env sh\necho encoded-checkout\n";
    await writeFile(join(root, "scripts/install.sh"), installer);
    const script = join(appRoot, "scripts/sync-install-script.mjs");
    await copyFile(new URL("./sync-install-script.mjs", import.meta.url), script);
    const result = spawnSync(process.execPath, ["scripts/sync-install-script.mjs"], {
      cwd: appRoot, encoding: "utf8",
    });
    assert.equal(result.status, 0, result.stdout + result.stderr);
    assert.match(result.stdout, /Synced/);
    assert.equal(await readFile(join(appRoot, "public/install.sh"), "utf8"), installer);
    assert.equal(await readFile(join(root, "install.sh"), "utf8"), installer);
    assert.equal((await stat(join(appRoot, "public/install.sh"))).mode & 0o111, 0o111);
  } finally { await rm(root, { recursive: true, force: true }); }
});
