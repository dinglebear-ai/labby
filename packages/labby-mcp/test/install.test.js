"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const {
  powershellExpandArchiveCommand,
  powershellLiteral,
} = require("../scripts/install");

test("quotes PowerShell literals for archive extraction", () => {
  assert.equal(powershellLiteral("C:\\Temp\\labby's.zip"), "'C:\\Temp\\labby''s.zip'");
});

test("builds Windows zip extraction command without PowerShell args", () => {
  const command = powershellExpandArchiveCommand(
    "C:\\Temp\\labby's.zip",
    "C:\\Users\\Docker\\vendor",
  );

  assert.match(command, /^Expand-Archive -LiteralPath /);
  assert.match(command, /'C:\\Temp\\labby''s.zip'/);
  assert.match(command, /'C:\\Users\\Docker\\vendor'/);
  assert.doesNotMatch(command, /\$args/);
});


test("npm archive extraction preserves binary and sibling companion directory", () => {
  const fs = require("node:fs"), os = require("node:os"), path = require("node:path");
  const { spawnSync } = require("node:child_process");
  const { extract } = require("../scripts/install");
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "labby-companion-extraction-"));
  try {
    const stage = path.join(tmp, "stage"), destination = path.join(tmp, "vendor");
    fs.mkdirSync(path.join(stage, "tailcat", "native"), { recursive: true });
    fs.writeFileSync(path.join(stage, "labby"), "binary");
    fs.writeFileSync(path.join(stage, "tailcat", "manifest.json"), '{"version":"fixture"}');
    fs.writeFileSync(path.join(stage, "tailcat", "native", "tailcat-bridge"), "helper");
    const archive = path.join(tmp, "release.tar.gz");
    assert.equal(spawnSync("tar", ["-czf", archive, "-C", stage, "labby", "tailcat"]).status, 0);
    extract(archive, destination, "tar.gz");
    assert.equal(fs.readFileSync(path.join(destination, "labby"), "utf8"), "binary");
    assert.equal(fs.readFileSync(path.join(destination, "tailcat", "native", "tailcat-bridge"), "utf8"), "helper");
    assert.equal(JSON.parse(fs.readFileSync(path.join(destination, "tailcat", "manifest.json"))).version, "fixture");
  } finally { fs.rmSync(tmp, { recursive: true, force: true }); }
});

test("npm installation repairs an interrupted companion extraction and validates before replacement", () => {
  const fs = require("node:fs"), os = require("node:os"), path = require("node:path"), crypto = require("node:crypto");
  const { spawnSync } = require("node:child_process");
  const { installArchive, installationValid, recoverInstallation } = require("../scripts/install");
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "labby-pair-install-"));
  const target = { binary: "labby", asset: "lab-aarch64-apple-darwin.tar.gz", archiveType: "tar.gz" };
  try {
    const stage = path.join(tmp, "release"), destination = path.join(tmp, "vendor");
    fs.mkdirSync(stage);
    fs.writeFileSync(path.join(stage, "labby"), "new binary");
    const components = [];
    for (const relative of ["native/tailcat-bridge", "browser/hook.mjs", "browser/transport.mjs", "browser/http.mjs", "browser/wasm.mjs", "browser/tailcat.wasm", "browser/tailcat.wasm.gz", "browser/wasm_exec.js", "browser/THIRD_PARTY_NOTICES.md", "adapter/server.mjs", "adapter/cleanup.mjs", "adapter/package.json", "adapter/package-lock.json"]) {
      const file = path.join(stage, "tailcat", relative);
      fs.mkdirSync(path.dirname(file), { recursive: true });
      fs.writeFileSync(file, relative);
      components.push({ path: relative, sha256: crypto.createHash("sha256").update(relative).digest("hex") });
    }
    fs.writeFileSync(path.join(stage, "tailcat", "manifest.json"), JSON.stringify({ schemaVersion: 1, protocol: 1, version: "2.5.0", target: "aarch64-apple-darwin", components }));
    const archive = path.join(tmp, "release.tar.gz");
    assert.equal(spawnSync("tar", ["-czf", archive, "-C", stage, "labby", "tailcat"]).status, 0);
    fs.mkdirSync(destination);
    fs.writeFileSync(path.join(destination, "labby"), "interrupted binary");
    assert.equal(installationValid(destination, target, "v2.5.0"), false);
    installArchive(archive, destination, target, "v2.5.0");
    assert.equal(installationValid(destination, target, "v2.5.0"), true);
    fs.renameSync(destination, destination + ".previous");
    recoverInstallation(destination);
    assert.equal(installationValid(destination, target, "v2.5.0"), true);
    fs.writeFileSync(path.join(stage, "tailcat", "adapter", "server.mjs"), "tampered");
    assert.equal(spawnSync("tar", ["-czf", archive, "-C", stage, "labby", "tailcat"]).status, 0);
    assert.throws(() => installArchive(archive, destination, target, "v2.5.0"), /checksum/);
    assert.equal(installationValid(destination, target, "v2.5.0"), true);
    const legacy = path.join(tmp, "legacy.tar.gz");
    assert.equal(spawnSync("tar", ["-czf", legacy, "-C", stage, "labby"]).status, 0);
    const older = path.join(tmp, "older");
    installArchive(legacy, older, target, "v2.4.0");
    assert.equal(installationValid(older, target, "v2.4.0"), true);
    assert.equal(installationValid(older, target, "v2.5.0"), false);
  } finally { fs.rmSync(tmp, { recursive: true, force: true }); }
});
